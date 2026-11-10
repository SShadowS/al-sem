//! **Configured dispatch** (callers-through-dependency design r2 §4,
//! `docs/superpowers/specs/2026-11-06-dependency-callers-design.md`): a site
//! whose targets are chosen at run time by configuration rows, while the
//! resolver sees its arms as ordinary static calls. Base App's workflow
//! response dispatch is the case: which response runs after which workflow
//! event is a program written in `Workflow Step Instance` rows.
//!
//! The rule is an explicit, engine-owned, versioned [`CATALOG`], never a shape
//! rule (a structural rule marks ordinary dispatch on a parameter; the
//! difference is one of meaning). An entry names a `case` statement in one
//! routine: app id (stable across versions), object, routine name and full
//! signature, and the scrutinee's member path. Its **configured region** is
//! that case's arm labels, arm bodies and `else` body ([`Region`]).
//!
//! The projection marks every site of the matched routine whose anchor line
//! lies in an arm with the site condition [`CONDITION`]
//! (`sited::CONDITIONS`). The site's boundary, `via` and confidence do not
//! change, and R0's walks do not read conditions: the condition **labels** a
//! real code path; it never hides one. The through-dependency walk
//! (`dep_callers`) reads it to rank a caller reachable only through such a
//! site after the others, and `alsem graph` exports it on the edge.
//!
//! **No fallback.** A matched routine with no such case, with more than one,
//! or whose case shares a source line with code outside it, is a
//! [`Status::Mismatch`]: nothing is labelled (never the whole body), and every
//! output that applies the catalog states it ([`CatalogReport`]). So does a
//! loaded app whose object or routine is missing or whose signature differs.
//! An app not loaded with source is [`Status::NotLoaded`].

use std::collections::{BTreeSet, HashMap};

use al_syntax::IdentifierFoldExt;
use al_syntax::ir::{AlFile, BlockId, BlockItem, ExprId, ExprKind, ObjectKind, StmtId, StmtKind};
use serde_json::{Value, json};

use crate::program::graph::ProgramGraph;
use crate::program::node::RoutineNodeId;
use crate::program::resolve::receiver::unquote_identifier;
use crate::snapshot::ParsedUnit;

/// The catalog's version: bumped whenever an entry is added, removed or
/// changed, so an output that changed because the catalog did says so.
pub const CATALOG_VERSION: &str = "1";

/// The site condition's name (in `sited::CONDITIONS`).
pub const CONDITION: &str = "configuredDispatch";

/// One catalogued configured-dispatch site.
#[derive(Debug)]
pub struct ConfiguredDispatch {
    /// How outputs name the entry.
    pub name: &'static str,
    /// The app's id (lower case); any version.
    pub app_id: &'static str,
    /// The app's name, reported, never matched.
    pub app_name: &'static str,
    pub object_kind: ObjectKind,
    /// The object's name, folded.
    pub object: &'static str,
    /// The routine's name, folded.
    pub routine: &'static str,
    /// Every parameter: `var`, and its type text normalized ([`normalize_type`]).
    pub params: &'static [(bool, &'static str)],
    /// The case's scrutinee as a member path, each part folded and unquoted.
    pub scrutinee: &'static [&'static str],
    /// Why its targets are configuration.
    pub why: &'static str,
}

/// The engine-owned catalog (design r2 §4.2). Verified against Base App
/// 28.1.49838.50268 `WorkflowResponseHandling.Codeunit.al`: the routine at
/// line 358, the case at 381, the custom-response event in the `else` at 453.
/// The same scrutinee text appears in `GetDescription` (line 1148), which is
/// not dispatch: the entry keys on routine and scrutinee together.
pub const CATALOG: &[ConfiguredDispatch] = &[ConfiguredDispatch {
    name: "Workflow Response Handling::ExecuteResponse",
    app_id: "437dbf0e-84ff-417a-965d-ed2bb9650972",
    app_name: "Base Application",
    object_kind: ObjectKind::Codeunit,
    object: "workflow response handling",
    routine: "executeresponse",
    params: &[
        (true, "variant"),
        (false, "record workflow step instance"),
        (false, "variant"),
    ],
    scrutinee: &["workflowresponse", "function name"],
    why: "responses are chosen by Workflow Step Instance rows",
}];

/// One arm of a configured region: its rows (0-based, inclusive; the label's
/// first row to the body's last) and its label as written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Arm {
    pub first_row: u32,
    pub last_row: u32,
    /// The arm's patterns as written (`PostDocumentCode()`), or `else`.
    pub label: String,
}

/// The configured region of one matched routine: its case's arms.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Region {
    /// The entry's index in [`CATALOG`].
    pub entry: u16,
    pub arms: Vec<Arm>,
}

impl Region {
    /// The arm whose rows hold `row` (a site anchor's 0-based start line).
    #[must_use]
    pub fn arm_at(&self, row: u32) -> Option<&Arm> {
        self.arms
            .iter()
            .find(|a| a.first_row <= row && row <= a.last_row)
    }
}

/// One entry's outcome in a program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// The app is not loaded with source: nothing to match.
    NotLoaded,
    /// Matched exactly one routine and one case.
    Matched {
        app_name: String,
        app_version: String,
    },
    /// The app is loaded, the shape is not the entry's: nothing labelled.
    Mismatch { reason: String },
}

impl Status {
    fn as_str(&self) -> &'static str {
        match self {
            Status::NotLoaded => "notLoaded",
            Status::Matched { .. } => "matched",
            Status::Mismatch { .. } => "mismatch",
        }
    }
}

/// Every entry's [`Status`], in [`CATALOG`] order.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct CatalogReport {
    pub entries: Vec<Status>,
}

impl CatalogReport {
    /// The JSON every output that applies the catalog carries.
    #[must_use]
    pub fn json(&self) -> Value {
        let entries: Vec<Value> = CATALOG
            .iter()
            .zip(&self.entries)
            .map(|(e, s)| {
                let mut m = serde_json::Map::new();
                m.insert("name".into(), e.name.into());
                m.insert("status".into(), s.as_str().into());
                match s {
                    Status::Matched {
                        app_name,
                        app_version,
                    } => {
                        m.insert("app".into(), app_name.as_str().into());
                        m.insert("appVersion".into(), app_version.as_str().into());
                    }
                    Status::Mismatch { reason } => {
                        m.insert("app".into(), e.app_name.into());
                        m.insert("reason".into(), reason.as_str().into());
                    }
                    Status::NotLoaded => {
                        m.insert("app".into(), e.app_name.into());
                    }
                }
                Value::Object(m)
            })
            .collect();
        json!({ "version": CATALOG_VERSION, "entries": entries })
    }

    /// `catalog 1: <entry> matched (<app> <version>); …`.
    #[must_use]
    pub fn human(&self) -> String {
        let parts: Vec<String> = CATALOG
            .iter()
            .zip(&self.entries)
            .map(|(e, s)| match s {
                Status::Matched {
                    app_name,
                    app_version,
                } => format!("{} matched ({app_name} {app_version})", e.name),
                Status::NotLoaded => {
                    format!("{} not loaded ({} has no source here)", e.name, e.app_name)
                }
                Status::Mismatch { reason } => {
                    format!("{} MISMATCH: {reason}; nothing labelled", e.name)
                }
            })
            .collect();
        format!("catalog {CATALOG_VERSION}: {}", parts.join("; "))
    }
}

/// A type text, normalized for signature matching: folded, double quotes
/// removed, runs of whitespace one space (`Record "Workflow Step Instance"`
/// -> `record workflow step instance`).
#[must_use]
pub fn normalize_type(text: &str) -> String {
    text.replace('"', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .fold_identifier()
}

/// A routine's signature, normalized: `(var, type)` per parameter.
fn signature(r: &al_syntax::ir::RoutineDecl) -> Vec<(bool, String)> {
    r.params
        .iter()
        .map(|p| (p.by_ref, normalize_type(p.ty.as_deref().unwrap_or(""))))
        .collect()
}

fn signature_text(sig: &[(bool, String)]) -> String {
    let parts: Vec<String> = sig
        .iter()
        .map(|(v, t)| if *v { format!("var {t}") } else { t.clone() })
        .collect();
    format!("({})", parts.join("; "))
}

/// An expression as a member path (`WorkflowResponse."Function Name"` ->
/// `["workflowresponse", "function name"]`), or `None` when it is not one.
fn member_path(file: &AlFile, e: ExprId) -> Option<Vec<String>> {
    match &file.ir.expr(e).kind {
        ExprKind::Identifier(s) | ExprKind::QuotedIdentifier(s) => {
            Some(vec![unquote_identifier(s).fold_identifier()])
        }
        ExprKind::Member { object, member, .. } => {
            let mut p = member_path(file, *object)?;
            p.push(unquote_identifier(member).fold_identifier());
            Some(p)
        }
        ExprKind::Parenthesized(inner) => member_path(file, *inner),
        _ => None,
    }
}

/// The source text of byte range `r`, whitespace runs folded to one space.
fn text_of(text: &str, r: &std::ops::Range<usize>) -> String {
    text.get(r.clone())
        .unwrap_or("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Every statement of `block` and its nested blocks, in source order.
fn each_stmt(file: &AlFile, block: BlockId, out: &mut Vec<StmtId>) {
    for item in &file.ir.block(block).items {
        match item {
            BlockItem::Stmt(s) => {
                out.push(*s);
                for b in child_blocks(file, *s) {
                    each_stmt(file, b, out);
                }
            }
            BlockItem::Preproc(g) => {
                for b in &g.branches {
                    each_stmt(file, *b, out);
                }
            }
        }
    }
}

/// The blocks a statement holds directly.
fn child_blocks(file: &AlFile, s: StmtId) -> Vec<BlockId> {
    match &file.ir.stmt(s).kind {
        StmtKind::If {
            then_block,
            else_block,
            ..
        } => std::iter::once(*then_block).chain(*else_block).collect(),
        StmtKind::Case {
            branches,
            else_block,
            ..
        } => branches.iter().map(|b| b.body).chain(*else_block).collect(),
        StmtKind::While { body, .. }
        | StmtKind::Repeat { body, .. }
        | StmtKind::For { body, .. }
        | StmtKind::Foreach { body, .. }
        | StmtKind::With { body, .. } => vec![*body],
        StmtKind::Try { body, catch_block } => std::iter::once(*body).chain(*catch_block).collect(),
        StmtKind::AssertError(b) | StmtKind::Block(b) => vec![*b],
        _ => Vec::new(),
    }
}

/// The arms of case statement `s` (a [`StmtKind::Case`]).
fn arms_of(file: &AlFile, text: &str, s: StmtId) -> Vec<Arm> {
    let StmtKind::Case {
        branches,
        else_block,
        ..
    } = &file.ir.stmt(s).kind
    else {
        return Vec::new();
    };
    let mut arms: Vec<Arm> = branches
        .iter()
        .map(|b| {
            let body = &file.ir.block(b.body).origin;
            let labels: Vec<String> = b
                .patterns
                .iter()
                .map(|p| text_of(text, &file.ir.expr(*p).origin.byte))
                .collect();
            let first = b
                .patterns
                .iter()
                .map(|p| file.ir.expr(*p).origin.start.row)
                .chain([b.origin.start.row, body.start.row])
                .min()
                .unwrap_or(b.origin.start.row);
            Arm {
                first_row: first,
                last_row: body.end.row.max(b.origin.end.row),
                label: labels.join(", "),
            }
        })
        .collect();
    if let Some(e) = else_block {
        let o = &file.ir.block(*e).origin;
        arms.push(Arm {
            first_row: o.start.row,
            last_row: o.end.row,
            label: "else".into(),
        });
    }
    arms
}

/// The configured region of `entry` in a routine whose identity and signature
/// already matched, or why the shape does not match (no fallback).
fn region_in(
    entry_ix: u16,
    entry: &ConfiguredDispatch,
    file: &AlFile,
    text: &str,
    body: Option<BlockId>,
) -> Result<Region, String> {
    let Some(body) = body else {
        return Err("the routine has no body".into());
    };
    let mut stmts = Vec::new();
    each_stmt(file, body, &mut stmts);
    let want: Vec<String> = entry.scrutinee.iter().map(|s| s.to_string()).collect();
    let cases: Vec<StmtId> = stmts
        .iter()
        .copied()
        .filter(|s| match &file.ir.stmt(*s).kind {
            StmtKind::Case { scrutinee, .. } => {
                member_path(file, *scrutinee).as_deref() == Some(want.as_slice())
            }
            _ => false,
        })
        .collect();
    let scrutinee = entry.scrutinee.join(".");
    let case = match cases.as_slice() {
        [] => return Err(format!("no `case {scrutinee} of` in the routine")),
        [one] => *one,
        many => {
            return Err(format!(
                "{} `case {scrutinee} of` statements in the routine",
                many.len()
            ));
        }
    };
    let arms = arms_of(file, text, case);
    let (Some(lo), Some(hi)) = (
        arms.iter().map(|a| a.first_row).min(),
        arms.iter().map(|a| a.last_row).max(),
    ) else {
        return Err(format!("`case {scrutinee} of` has no arm"));
    };
    // The case's own statements and the statements that hold it may share
    // its rows; any other statement on those rows makes the region, decided
    // by lines, ambiguous.
    let mut inside: BTreeSet<usize> = BTreeSet::new();
    let mut nested = Vec::new();
    for b in child_blocks(file, case) {
        each_stmt(file, b, &mut nested);
    }
    inside.extend(nested.iter().map(|s| s.index()));
    inside.insert(case.index());
    let holders: BTreeSet<usize> = stmts
        .iter()
        .copied()
        .filter(|s| {
            let mut sub = Vec::new();
            for b in child_blocks(file, *s) {
                each_stmt(file, b, &mut sub);
            }
            sub.contains(&case)
        })
        .map(|s| s.index())
        .collect();
    for s in &stmts {
        if inside.contains(&s.index()) || holders.contains(&s.index()) {
            continue;
        }
        let o = &file.ir.stmt(*s).origin;
        if o.start.row <= hi && o.end.row >= lo {
            return Err(format!(
                "`case {scrutinee} of` shares source line {} with a statement outside it",
                o.start.row.max(lo) + 1
            ));
        }
    }
    Ok(Region {
        entry: entry_ix,
        arms,
    })
}

/// What one entry found while the units were read.
#[derive(Default)]
struct Found {
    /// The app's `(name, version)`, when a source unit of it was read.
    app: Option<(String, String)>,
    object: bool,
    /// Routines with the entry's name whose signature differs.
    other_signatures: Vec<String>,
    /// Routines with the entry's name and signature, and their regions.
    routines: Vec<(RoutineNodeId, Result<Region, String>)>,
}

/// The configured regions of every matched routine, and the catalog's report,
/// from every source unit (`catalog` is [`CATALOG`] outside tests).
#[must_use]
pub fn extract(
    catalog: &[ConfiguredDispatch],
    graph: &ProgramGraph,
    units: &[&ParsedUnit],
) -> (HashMap<RoutineNodeId, Region>, CatalogReport) {
    let mut found: Vec<Found> = catalog.iter().map(|_| Found::default()).collect();
    for unit in units {
        let entries: Vec<usize> = catalog
            .iter()
            .enumerate()
            .filter(|(_, e)| unit.app.guid.eq_ignore_ascii_case(e.app_id))
            .map(|(i, _)| i)
            .collect();
        if entries.is_empty() {
            continue;
        }
        let Some(app) = graph.apps.find(&unit.app) else {
            continue;
        };
        for &i in &entries {
            found[i]
                .app
                .get_or_insert_with(|| (unit.app.name.clone(), unit.app.version.clone()));
        }
        for pf in &unit.files {
            let decl_nodes = crate::program::split_decls::file_declaration_nodes(
                graph,
                &pf.file,
                app,
                &crate::program::split_decls::unit_of_parsed(pf),
            );
            for (obj_ix, obj) in pf.file.objects.iter().enumerate() {
                for &i in &entries {
                    let e = &catalog[i];
                    if obj.kind != e.object_kind
                        || unquote_identifier(&obj.name).fold_identifier() != e.object
                    {
                        continue;
                    }
                    found[i].object = true;
                    for (routine_ix, r) in obj.routines.iter().enumerate() {
                        if r.name.fold_identifier() != e.routine || r.enclosing_member.is_some() {
                            continue;
                        }
                        let sig = signature(r);
                        let want: Vec<(bool, String)> =
                            e.params.iter().map(|(v, t)| (*v, t.to_string())).collect();
                        if sig != want {
                            found[i].other_signatures.push(signature_text(&sig));
                            continue;
                        }
                        let region = region_in(i as u16, e, &pf.file, &pf.text, r.body);
                        found[i]
                            .routines
                            .push((decl_nodes[obj_ix][routine_ix].clone(), region));
                    }
                }
            }
        }
    }
    let mut regions = HashMap::new();
    let mut report = CatalogReport::default();
    for (e, f) in catalog.iter().zip(found) {
        let status = match (&f.app, f.object, f.routines.len()) {
            (None, _, _) => Status::NotLoaded,
            (Some(_), false, _) => Status::Mismatch {
                reason: format!("no {:?} named {:?} in the app", e.object_kind, e.object),
            },
            (Some(_), true, 0) if f.other_signatures.is_empty() => Status::Mismatch {
                reason: format!("no routine {:?} in the object", e.routine),
            },
            (Some(_), true, 0) => Status::Mismatch {
                reason: format!(
                    "the signature differs: found {}, catalogued {}",
                    f.other_signatures.join(", "),
                    signature_text(
                        &e.params
                            .iter()
                            .map(|(v, t)| (*v, t.to_string()))
                            .collect::<Vec<_>>()
                    )
                ),
            },
            (Some(_), true, 1) => {
                let (id, region) = f.routines.into_iter().next().expect("one routine");
                match region {
                    Ok(region) => {
                        regions.insert(id, region);
                        let (app_name, app_version) = f.app.clone().expect("app seen");
                        Status::Matched {
                            app_name,
                            app_version,
                        }
                    }
                    Err(reason) => Status::Mismatch { reason },
                }
            }
            (Some(_), true, n) => Status::Mismatch {
                reason: format!("{n} routines match the name and signature"),
            },
        };
        report.entries.push(status);
    }
    (regions, report)
}

/// One `case` in dependency code, for the discovery probe
/// (`aldump --configured-dispatch-candidates`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// `Codeunit Workflow Response Handling::ExecuteResponse`.
    pub routine: String,
    /// The scrutinee as written.
    pub scrutinee: String,
    /// The case's arms (the `else` counts as one).
    pub arms: usize,
    /// The arms with at least one code-call site of the routine.
    pub call_arms: usize,
    /// The distinct targets of those sites.
    pub callees: usize,
    /// The catalog labels the case's sites.
    pub catalogued: bool,
}

/// Every `case` in the dependency units of `g`'s program with at least
/// `min_arms` arms making a code call (a resolved code-call site of the
/// routine whose line lies in the arm), most such arms first.
#[must_use]
pub fn candidates(
    g: &super::SitedGraph,
    graph: &ProgramGraph,
    units: &[&ParsedUnit],
    min_arms: usize,
) -> Vec<Candidate> {
    let mut out_sites: HashMap<u32, Vec<u32>> = HashMap::new();
    for (i, s) in g.sites().iter().enumerate() {
        if super::reverse::is_code_call_site(g, i as u32) {
            out_sites.entry(s.from).or_default().push(i as u32);
        }
    }
    let mut out = Vec::new();
    for unit in units {
        let Some(app) = graph.apps.find(&unit.app) else {
            continue;
        };
        if app == g.primary_app() {
            continue;
        }
        for pf in &unit.files {
            let decl_nodes = crate::program::split_decls::file_declaration_nodes(
                graph,
                &pf.file,
                app,
                &crate::program::split_decls::unit_of_parsed(pf),
            );
            for (obj_ix, obj) in pf.file.objects.iter().enumerate() {
                for (routine_ix, r) in obj.routines.iter().enumerate() {
                    let Some(body) = r.body else { continue };
                    let Some(node) = g.routine_node(&decl_nodes[obj_ix][routine_ix]) else {
                        continue;
                    };
                    let sites = out_sites.get(&node).map(Vec::as_slice).unwrap_or(&[]);
                    let mut stmts = Vec::new();
                    each_stmt(&pf.file, body, &mut stmts);
                    for s in stmts {
                        let StmtKind::Case { scrutinee, .. } = &pf.file.ir.stmt(s).kind else {
                            continue;
                        };
                        let arms = arms_of(&pf.file, &pf.text, s);
                        let mut callees: BTreeSet<u32> = BTreeSet::new();
                        let mut call_arms = 0;
                        let mut catalogued = false;
                        for a in &arms {
                            let in_arm: Vec<u32> = sites
                                .iter()
                                .copied()
                                .filter(|&x| {
                                    g.sites()[x as usize].anchor.is_some_and(|an| {
                                        a.first_row <= an.start_line && an.start_line <= a.last_row
                                    })
                                })
                                .collect();
                            if !in_arm.is_empty() {
                                call_arms += 1;
                            }
                            for x in in_arm {
                                callees.insert(g.sites()[x as usize].to);
                                catalogued |= g.configured_arm(x).is_some();
                            }
                        }
                        if call_arms < min_arms {
                            continue;
                        }
                        out.push(Candidate {
                            routine: format!(
                                "{} {}::{}",
                                crate::program::graphify_export::object_kind_str(obj.kind),
                                obj.name,
                                r.name
                            ),
                            scrutinee: text_of(&pf.text, &pf.file.ir.expr(*scrutinee).origin.byte),
                            arms: arms.len(),
                            call_arms,
                            callees: callees.len(),
                            catalogued,
                        });
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| {
        b.call_arms
            .cmp(&a.call_arms)
            .then_with(|| a.routine.cmp(&b.routine))
            .then_with(|| a.scrutinee.cmp(&b.scrutinee))
    });
    out
}

/// `aldump --configured-dispatch-candidates`: the catalog's report and the
/// candidates of a cross-app program (its sited graph is built here).
pub fn probe(
    program: &crate::program::resolved::ResolvedProgram,
    min_arms: usize,
) -> Result<(CatalogReport, Vec<Candidate>), super::Cancelled> {
    let g = super::sited_from_program(program, &|| false)?;
    let ctx = &program.body().ctx;
    use crate::program::profile::DependencyBodies;
    let units: Vec<&ParsedUnit> = match ctx.profile().dependency_bodies {
        DependencyBodies::Summary => ctx.parsed().iter().collect(),
        DependencyBodies::OnDemand | DependencyBodies::Keep => ctx.all_units(),
    };
    let found = candidates(&g, ctx.graph(), &units, min_arms);
    Ok((g.catalog().clone(), found))
}

#[cfg(test)]
#[path = "configured_dispatch_tests.rs"]
mod tests;
