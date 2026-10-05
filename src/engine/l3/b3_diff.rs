//! B3 Phase A, Task 4: the detector difference harness (code map C10).
//!
//! Runs every registered detector (opt-in ones too) twice over ONE
//! `L3Resolved`: first with L3's own call resolution
//! (`precomputed_calls = None`), then with the program-engine adapter's
//! ([`resolved_calls_with_notes`]). Routine ids, root-cause keys and
//! fingerprints are therefore directly comparable. Findings are diffed by
//! `(detector, primary location, root cause key)` into `removed` / `added` /
//! `changed`, and each difference is attributed to the call sites inside the
//! finding's routines (primary + evidence path) whose old and new edges
//! differ, with the adapter's census category for each site. The output is a
//! markdown triage table with an empty verdict column (Task 5 fills it).
//!
//! Diagnostic only: nothing here changes production behaviour.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;

use crate::engine::l3::call_resolver::{CallEdge, ResolvedCalls, UpgradedBinding, resolve_calls};
use crate::engine::l3::l3_workspace::L3Workspace;
use crate::engine::l3::program_calls::{
    SiteCensus, SiteNote, SiteNotes, build_models, resolved_calls_with_notes,
};
use crate::engine::l3::symbol_table::SymbolTable;
use crate::engine::l5::detectors::registered_detectors;
use crate::engine::l5::finding::{EvidenceStep, Finding, evidence_path_of};
use crate::engine::l5::registry::run_detectors;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DiffKind {
    Removed,
    Added,
    Changed,
}

impl DiffKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DiffKind::Removed => "removed",
            DiffKind::Added => "added",
            DiffKind::Changed => "changed",
        }
    }
}

/// One finding difference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindingDiff {
    pub kind: DiffKind,
    pub detector: String,
    /// `unit:line:col`, 1-based.
    pub location: String,
    pub root_cause_key: String,
    pub what: String,
    /// The finding's routines (primary + every realizing path, both
    /// versions), sorted.
    pub routines: Vec<String>,
    /// Attribution only: `Some("callers" | "callees")` when the attributed
    /// sites were found through the finding's transitive callers or callees,
    /// not in or into its own routines.
    pub via: Option<&'static str>,
}

/// One call site or record op whose old and new edges (or bindings) differ.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteDiff {
    pub routine: String,
    /// Every routine an old or new edge targets (`to` and candidates).
    pub targets: Vec<String>,
    /// `unit:line:col`, 1-based.
    pub location: String,
    pub callee: String,
    pub old: String,
    pub new: String,
    pub program: String,
    pub categories: Vec<String>,
}

/// The harness result for one workspace.
#[derive(Debug, Clone)]
pub struct DetectorDiff {
    /// `--b3-deps`: old is the adapter without stage 2, new the adapter
    /// with it; otherwise old is L3's own resolution.
    pub deps: bool,
    pub findings_old: usize,
    pub findings_new: usize,
    pub diagnostics_old: usize,
    pub diagnostics_new: usize,
    /// Each difference with the indexes of its attributed [`SiteDiff`]s.
    pub rows: Vec<(FindingDiff, Vec<usize>)>,
    pub sites: Vec<SiteDiff>,
    pub census: SiteCensus,
    /// The two finding sets themselves (`new` is the reference `alsem
    /// analyze` is tested against).
    pub old: Vec<Finding>,
    pub new: Vec<Finding>,
}

fn loc(unit: &str, line: u32, col: u32) -> String {
    format!(
        "{}:{}:{}",
        unit.strip_prefix("ws:").unwrap_or(unit),
        line + 1,
        col + 1
    )
}

type FindingKey<'a> = (&'a str, &'a str, u32, u32, &'a str);

fn key(f: &Finding) -> FindingKey<'_> {
    let a = &f.primary_location;
    (
        &f.detector,
        &a.source_unit_id,
        a.start_line,
        a.start_column,
        &f.root_cause_key,
    )
}

fn routines_of<'a>(fs: impl IntoIterator<Item = &'a Finding>) -> Vec<String> {
    let mut set = BTreeSet::new();
    for f in fs {
        set.insert(f.primary_location.enclosing_routine_id.clone());
        for s in evidence_path_of(f).iter() {
            set.insert(s.routine_id.clone());
        }
        // Every other realizing path: d1's other cohorts, others' additional paths.
        for c in f.cohort_contexts.iter().flatten() {
            for s in crate::engine::l5::d1_witness::flatten_witness(&c.witness) {
                set.insert(s.routine_id);
            }
        }
        for s in f.additional_paths.iter().flatten().flatten() {
            set.insert(s.routine_id.clone());
        }
    }
    set.remove("");
    set.into_iter().collect()
}

fn summary(f: &Finding) -> String {
    format!("{} / {}: {}", f.severity, f.confidence.level, f.title)
}

/// What differs between two findings with the same key; empty when nothing
/// the triage cares about does.
fn changes(o: &Finding, n: &Finding) -> Vec<String> {
    let mut out = Vec::new();
    if o.severity != n.severity {
        out.push(format!("severity {} -> {}", o.severity, n.severity));
    }
    if o.confidence.level != n.confidence.level {
        out.push(format!(
            "confidence {} -> {}",
            o.confidence.level, n.confidence.level
        ));
    }
    if o.confidence.capped_by != n.confidence.capped_by {
        out.push(format!(
            "cappedBy {:?} -> {:?}",
            o.confidence.capped_by.as_deref().unwrap_or_default(),
            n.confidence.capped_by.as_deref().unwrap_or_default()
        ));
    }
    let (op, np) = (evidence_path_of(o), evidence_path_of(n));
    if op != np {
        let i = op.iter().zip(np.iter()).take_while(|(a, b)| a == b).count();
        let step = |p: &[EvidenceStep]| {
            p.get(i).map_or("(none)".to_string(), |s| {
                let a = &s.source_anchor;
                format!(
                    "{} {}",
                    loc(&a.source_unit_id, a.start_line, a.start_column),
                    s.note
                )
            })
        };
        out.push(format!(
            "evidence path ({} -> {} steps), first difference at step {}: {} -> {}",
            op.len(),
            np.len(),
            i + 1,
            step(&op),
            step(&np)
        ));
    }
    if o.title != n.title {
        out.push(format!("title {}", text_change(&o.title, &n.title)));
    }
    if o.root_cause != n.root_cause {
        out.push(format!(
            "rootCause {}",
            text_change(&o.root_cause, &n.root_cause)
        ));
    }
    out
}

/// The first differing span of two texts, with a little context:
/// `"…a old b…" -> "…a new b…"`.
fn text_change(old: &str, new: &str) -> String {
    const CONTEXT: usize = 20;
    const MAX: usize = 60;
    let prefix = old
        .char_indices()
        .zip(new.chars())
        .take_while(|((_, a), b)| a == b)
        .last()
        .map_or(0, |((i, c), _)| i + c.len_utf8());
    let suffix = old[prefix..]
        .chars()
        .rev()
        .zip(new[prefix..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(c, _)| c.len_utf8())
        .sum::<usize>();
    let start = old[..prefix]
        .char_indices()
        .rev()
        .nth(CONTEXT - 1)
        .map_or(0, |(i, _)| i);
    let excerpt = |s: &str| {
        let end = s.len() - suffix;
        let body: String = s[start..end].chars().take(MAX).collect();
        let more = s[start..end].chars().count() > MAX || end < s.len();
        format!(
            "\"{}{body}{}\"",
            if start > 0 { "…" } else { "" },
            if more { "…" } else { "" }
        )
    };
    format!("{} -> {}", excerpt(old), excerpt(new))
}

/// Diff two finding sets by `(detector, primary location, root cause key)`.
/// Same-key findings pair in run order; the surplus on either side is
/// `removed` / `added`. Output sorted by key, then kind.
#[must_use]
pub fn diff_findings(old: &[Finding], new: &[Finding]) -> Vec<FindingDiff> {
    let mut groups: BTreeMap<FindingKey<'_>, (Vec<&Finding>, Vec<&Finding>)> = BTreeMap::new();
    for f in old {
        groups.entry(key(f)).or_default().0.push(f);
    }
    for f in new {
        groups.entry(key(f)).or_default().1.push(f);
    }
    let mut out = Vec::new();
    for ((det, unit, line, col, rck), (os, ns)) in groups {
        let row = |kind, what: String, fs: &[&Finding]| FindingDiff {
            kind,
            detector: det.to_string(),
            location: loc(unit, line, col),
            root_cause_key: rck.to_string(),
            what,
            routines: routines_of(fs.iter().copied()),
            via: None,
        };
        for i in 0..os.len().max(ns.len()) {
            match (os.get(i), ns.get(i)) {
                (Some(o), Some(n)) => {
                    let c = changes(o, n);
                    if !c.is_empty() {
                        out.push(row(DiffKind::Changed, c.join("; "), &[o, n]));
                    }
                }
                (Some(o), None) => out.push(row(DiffKind::Removed, summary(o), &[o])),
                (None, Some(n)) => out.push(row(DiffKind::Added, summary(n), &[n])),
                (None, None) => unreachable!(),
            }
        }
    }
    out
}

/// Routine id → `Object.Routine`, for readable edge targets.
type Names<'a> = HashMap<&'a str, String>;

fn routine_names(ws: &L3Workspace) -> Names<'_> {
    let objects: HashMap<&str, &str> = ws
        .objects
        .iter()
        .map(|o| (o.id.as_str(), o.name.as_str()))
        .collect();
    ws.routines
        .iter()
        .map(|r| {
            let obj = objects.get(r.object_id.as_str()).copied().unwrap_or("?");
            (r.id.as_str(), format!("{} {obj}.{}", r.object_type, r.name))
        })
        .collect()
}

fn render_edges(edges: &[&CallEdge], names: &Names<'_>) -> String {
    if edges.is_empty() {
        return "(no edge)".to_string();
    }
    edges
        .iter()
        .map(|e| {
            let mut s = format!("{:?}/{:?}", e.dispatch_kind, e.resolution);
            if let Some(to) = &e.to {
                let name = names.get(to.as_str()).map_or(to.as_str(), String::as_str);
                s.push_str(&format!(" -> {name}"));
            }
            if let Some(r) = &e.receiver_shape {
                s.push_str(&format!(" shape {r}"));
            }
            if let Some(m) = &e.unknown_method_name {
                s.push_str(&format!(" method {m}"));
            }
            if let Some(m) = &e.dispatch_meta {
                s.push_str(&format!(
                    " meta {} {}/{}",
                    m.interface_name,
                    m.unresolved_impls.len(),
                    m.total_impls
                ));
            }
            if let Some(t) = &e.external_type_ref {
                s.push_str(&format!(" ext {} {}", t.kind, t.name));
            }
            if let Some(c) = &e.candidates {
                s.push_str(&format!(" candidates {}", c.len()));
            }
            if let Some(r) = &e.receiver_type {
                s.push_str(&format!(" recv {r}"));
            }
            s
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// The plain `program-site` / `program-trigger` category split by the
/// first edge's `dispatch/resolution` transition, so the triage can group
/// the bulk of the differences by what moved. Other categories unchanged.
fn refine(categories: &[String], old: &[&CallEdge], new: &[&CallEdge]) -> Vec<String> {
    let [only] = categories else {
        return categories.to_vec();
    };
    if only != "program-site" && only != "program-trigger" {
        return categories.to_vec();
    }
    let shape = |es: &[&CallEdge]| {
        es.first().map_or("no edge".to_string(), |e| {
            format!("{:?}/{:?}", e.dispatch_kind, e.resolution)
        })
    };
    let (o, n) = (shape(old), shape(new));
    let what = if o != n {
        format!("{o} -> {n}")
    } else if old.len() != new.len() {
        format!("{o}, {} -> {} edges", old.len(), new.len())
    } else if old == new {
        format!("{o}, bindings")
    } else {
        format!("{o}, details")
    };
    vec![format!("{only}: {what}")]
}

fn render_bindings(b: Option<&Vec<UpgradedBinding>>) -> String {
    let Some(b) = b else {
        return "none".to_string();
    };
    let parts: Vec<String> = b
        .iter()
        .map(|x| {
            format!(
                "{}:{}{}",
                x.parameter_index,
                if x.callee_parameter_is_var {
                    "var "
                } else {
                    ""
                },
                x.binding_resolution
            )
        })
        .collect();
    format!("[{}]", parts.join(", "))
}

/// Every call site and record op whose edges or bindings differ between
/// `old` and `new`, in routine/site order.
fn site_diffs(
    ws: &L3Workspace,
    old: &ResolvedCalls,
    new: &ResolvedCalls,
    notes: &SiteNotes,
) -> Vec<SiteDiff> {
    fn group(rc: &ResolvedCalls) -> HashMap<(&str, &str), Vec<&CallEdge>> {
        let mut m: HashMap<(&str, &str), Vec<&CallEdge>> = HashMap::new();
        for e in &rc.edges {
            m.entry((e.from.as_str(), e.callsite_id.as_str()))
                .or_default()
                .push(e);
        }
        m
    }
    let (og, ng) = (group(old), group(new));
    let names = routine_names(ws);
    let none = Vec::new();
    let mut out = Vec::new();
    for r in &ws.routines {
        let sites = r
            .call_sites
            .iter()
            .map(|cs| (&cs.id, &cs.source_anchor, cs.callee_text.clone(), true))
            .chain(r.record_operations.iter().map(|op| {
                (
                    &op.id,
                    &op.source_anchor,
                    format!("{}.{}", op.record_variable_name, op.op),
                    false,
                )
            }));
        for (id, a, callee, is_call) in sites {
            let k = (r.id.as_str(), id.as_str());
            let (oe, ne) = (og.get(&k).unwrap_or(&none), ng.get(&k).unwrap_or(&none));
            let (ob, nb) = if is_call {
                (old.upgraded_bindings.get(id), new.upgraded_bindings.get(id))
            } else {
                (None, None)
            };
            if oe == ne && ob == nb {
                continue;
            }
            let mut old_s = render_edges(oe, &names);
            let mut new_s = render_edges(ne, &names);
            if ob != nb {
                old_s.push_str(&format!(" bindings {}", render_bindings(ob)));
                new_s.push_str(&format!(" bindings {}", render_bindings(nb)));
            }
            let mut targets: Vec<String> = oe
                .iter()
                .chain(ne.iter())
                .flat_map(|e| e.to.iter().chain(e.candidates.iter().flatten()))
                .cloned()
                .collect();
            targets.sort();
            targets.dedup();
            let note = notes.get(&(r.id.clone(), id.clone()));
            out.push(SiteDiff {
                routine: r.id.clone(),
                targets,
                location: loc(&a.source_unit_id, a.start_line, a.start_column),
                callee,
                old: old_s,
                new: new_s,
                program: note
                    .and_then(|n| n.program.clone())
                    .unwrap_or_else(|| "-".to_string()),
                categories: note.map_or_else(
                    || vec!["no-note".to_string()],
                    |n: &SiteNote| refine(&n.categories, oe, ne),
                ),
            });
        }
    }
    out
}

/// Attach to each difference the differing sites inside its routines, and
/// the differing sites whose old or new edges target one of them (a
/// reachability finding such as d14's moves with its callers' edges).
///
/// When that finds none, two transitive fallbacks over `edges` (`(from,
/// to)`, old and new edges together), marked in [`FindingDiff::via`]:
/// `callers` — differing sites whose edges target a routine that reaches
/// the finding's routines; else `callees` — differing sites inside a
/// routine the finding's routines reach (a summary-driven finding, e.g. a
/// transitive `Commit`, moves with its callees' edges).
fn attribute(
    diffs: Vec<FindingDiff>,
    sites: &[SiteDiff],
    edges: &[(String, String)],
) -> Vec<(FindingDiff, Vec<usize>)> {
    let mut by_routine: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut by_target: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, s) in sites.iter().enumerate() {
        by_routine.entry(s.routine.as_str()).or_default().push(i);
        for t in &s.targets {
            by_target.entry(t.as_str()).or_default().push(i);
        }
    }
    let mut callers: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut callees: HashMap<&str, Vec<&str>> = HashMap::new();
    for (from, to) in edges {
        callers.entry(to).or_default().push(from);
        callees.entry(from).or_default().push(to);
    }
    let lookup = |m: &HashMap<&str, Vec<usize>>, rs: &[&str]| {
        let mut ix: Vec<usize> = rs
            .iter()
            .flat_map(|r| m.get(r).into_iter().flatten().copied())
            .collect();
        ix.sort_unstable();
        ix.dedup();
        ix
    };
    fn closure<'a>(start: &[&'a str], g: &HashMap<&'a str, Vec<&'a str>>) -> Vec<&'a str> {
        let mut seen: BTreeSet<&str> = start.iter().copied().collect();
        let mut stack: Vec<&str> = start.to_vec();
        while let Some(r) = stack.pop() {
            for &n in g.get(r).into_iter().flatten() {
                if seen.insert(n) {
                    stack.push(n);
                }
            }
        }
        seen.into_iter().collect()
    }
    diffs
        .into_iter()
        .map(|mut d| {
            let own: Vec<&str> = d.routines.iter().map(String::as_str).collect();
            let mut ix = lookup(&by_routine, &own);
            ix.extend(lookup(&by_target, &own));
            ix.sort_unstable();
            ix.dedup();
            if ix.is_empty() {
                ix = lookup(&by_target, &closure(&own, &callers));
                d.via = (!ix.is_empty()).then_some("callers");
            }
            if ix.is_empty() {
                ix = lookup(&by_routine, &closure(&own, &callees));
                d.via = (!ix.is_empty()).then_some("callees");
            }
            (d, ix)
        })
        .collect()
}

/// Run the harness over one workspace (code map C10). Both models are
/// resident together; this is a diagnostic, not the `analyze` path.
///
/// `deps` (`aldump --b3 --b3-deps`, Task 6): diff the adapter WITHOUT its
/// stage 2 (dependency-callee bindings, `upgrade_dependency_bindings =
/// false`) against the adapter WITH it, so that effect is triaged alone.
/// Otherwise old is L3's own resolution and new the adapter with stage 2.
pub fn detector_diff_for_workspace(workspace: &Path, deps: bool) -> Result<DetectorDiff, String> {
    let (ctx, report, mut l3) = build_models(workspace)?;
    let detectors = registered_detectors();
    let old_calls = if deps {
        Some(resolved_calls_with_notes(&report, &ctx, &l3.workspace, false).0)
    } else {
        None
    };
    if let Some(calls) = &old_calls {
        l3.precomputed_calls = Some(Arc::new(calls.clone()));
    }
    let old_out = run_detectors(&l3, &detectors);
    let (new_calls, census, sites, edges) = {
        let ws = &l3.workspace;
        let old_calls = old_calls.unwrap_or_else(|| {
            let symbols = SymbolTable::build(&ws.objects, &ws.tables, &ws.routines);
            resolve_calls(ws, &symbols, &[], &[])
        });
        let (new_calls, census, notes) = resolved_calls_with_notes(&report, &ctx, ws, true);
        let sites = site_diffs(ws, &old_calls, &new_calls, &notes);
        let mut edges: Vec<(String, String)> = old_calls
            .edges
            .iter()
            .chain(&new_calls.edges)
            .filter_map(|e| Some((e.from.clone(), e.to.clone()?)))
            .collect();
        edges.sort();
        edges.dedup();
        (new_calls, census, sites, edges)
    };
    drop((ctx, report));
    l3.precomputed_calls = Some(Arc::new(new_calls));
    let new_out = run_detectors(&l3, &detectors);
    let rows = attribute(
        diff_findings(&old_out.findings, &new_out.findings),
        &sites,
        &edges,
    );
    Ok(DetectorDiff {
        deps,
        findings_old: old_out.findings.len(),
        findings_new: new_out.findings.len(),
        diagnostics_old: old_out.diagnostics.len(),
        diagnostics_new: new_out.diagnostics.len(),
        rows,
        sites,
        census,
        old: old_out.findings,
        new: new_out.findings,
    })
}

fn cell(s: &str) -> String {
    s.replace('|', "\\|").replace(['\n', '\r'], " ")
}

/// The attribution category of a difference: its sites' categories, joined,
/// prefixed `via callers:` / `via callees:` for a transitive attribution.
fn row_category(sites: &[SiteDiff], d: &FindingDiff, ix: &[usize]) -> String {
    let cats: BTreeSet<&str> = ix
        .iter()
        .flat_map(|&i| sites[i].categories.iter().map(String::as_str))
        .collect();
    if cats.is_empty() {
        return "unattributed".to_string();
    }
    let joined = cats.into_iter().collect::<Vec<_>>().join(" + ");
    match d.via {
        Some(via) => format!("via {via}: {joined}"),
        None => joined,
    }
}

/// A difference-table row (7 cells): `(row without its verdict cell,
/// verdict)`. `None` for any other line. `\|` inside a cell is not a
/// separator.
fn split_verdict(line: &str) -> Option<(&str, &str)> {
    if !line.starts_with('|') || !line.ends_with('|') {
        return None;
    }
    let bytes = line.as_bytes();
    let seps: Vec<usize> = (0..bytes.len())
        .filter(|&i| bytes[i] == b'|' && (i == 0 || bytes[i - 1] != b'\\'))
        .collect();
    if seps.len() != 8 {
        return None;
    }
    let cut = seps[6];
    Some((&line[..cut], line[cut + 1..seps[7]].trim()))
}

/// `md` with the verdict cell dropped from every difference-table row, for
/// comparing two tables regardless of their verdicts.
#[must_use]
pub fn strip_verdicts(md: &str) -> String {
    md.lines()
        .map(|l| split_verdict(l).map_or(l, |(head, _)| head))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `new` (a freshly rendered table) with the verdicts of `old` (the file it
/// replaces) carried over: a difference row whose text without the verdict
/// is unchanged keeps its non-empty verdict; rows that no longer exist drop
/// theirs; new rows stay empty.
#[must_use]
pub fn carry_verdicts(new: &str, old: &str) -> String {
    let old = old.replace("\r\n", "\n");
    let verdicts: HashMap<&str, &str> = old
        .lines()
        .filter_map(split_verdict)
        .filter(|(_, v)| !v.is_empty())
        .collect();
    let mut out: String = new
        .lines()
        .map(|l| match split_verdict(l) {
            Some((head, "")) => match verdicts.get(head) {
                Some(v) => format!("{head}| {v} |"),
                None => l.to_string(),
            },
            _ => l.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n");
    if new.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// The markdown triage table for one or more workspaces. `prefix`ed
/// locations (`<name>/…`) are used when there is more than one workspace.
/// `errors` lists workspaces whose models could not be built.
#[must_use]
pub fn triage_markdown(
    title: &str,
    corpora: &[(String, DetectorDiff)],
    errors: &[(String, String)],
) -> String {
    use std::fmt::Write;
    let multi = corpora.len() + errors.len() > 1;
    let pre = |name: &str, s: &str| {
        if multi {
            format!("{name}/{s}")
        } else {
            s.to_string()
        }
    };
    let deps = corpora.first().is_some_and(|(_, d)| d.deps);
    let (old_name, new_name) = if deps {
        ("adapter, stage 1", "adapter, stage 2")
    } else {
        ("L3", "adapter")
    };
    let mut md = String::new();
    let _ = writeln!(md, "# B3 detector diff: {title}\n");
    let runs = if deps {
        "Generated by the B3 Phase A harness (`aldump --b3 <ws> --b3-deps --b3-triage <file>`; \
         `src/engine/l3/b3_diff.rs`). Every registered detector (opt-in included) ran twice \
         over one `L3Resolved`: with the program-engine adapter WITHOUT its stage 2 (old: a \
         dependency callee's argument bindings stay \"unresolved-callee\") and WITH it (new: \
         they are upgraded with the dependency routine's parameter var-ness), so only that \
         effect differs."
    } else {
        "Generated by the B3 Phase A harness (`aldump --b3 <ws> --b3-triage <file>`; \
         `src/engine/l3/b3_diff.rs`). Every registered detector (opt-in included) ran twice \
         over one `L3Resolved`: with L3's call resolution (old) and with the program-engine \
         adapter's (new)."
    };
    let _ = writeln!(
        md,
        "{runs} Findings are keyed by (detector, primary location, root cause key). \
         \"Attributed sites\" are the call sites / record ops inside the finding's routines \
         (primary + every realizing path), or whose edges target them, where the old and new \
         edges differ, with the adapter's census category. When none exist, `via callers:` \
         takes the differing sites whose edges target a routine that reaches the finding's \
         routines, and `via callees:` the differing sites inside routines the finding's \
         routines reach; a category joined with `+` is the union of all attributed sites' \
         categories, not a single cause. Verdict: fixed / regression / unexplained (Task 5).\n"
    );

    // Summary.
    let (mut fo, mut fnew, mut dold, mut dnew, mut nsites) = (0, 0, 0, 0, 0);
    let mut by_kind: BTreeMap<DiffKind, usize> = BTreeMap::new();
    let mut matrix: BTreeMap<(String, DiffKind, String), usize> = BTreeMap::new();
    let mut site_cats: BTreeMap<String, usize> = BTreeMap::new();
    for (_, d) in corpora {
        fo += d.findings_old;
        fnew += d.findings_new;
        dold += d.diagnostics_old;
        dnew += d.diagnostics_new;
        nsites += d.sites.len();
        for s in &d.sites {
            *site_cats.entry(s.categories.join(" + ")).or_default() += 1;
        }
        for (r, ix) in &d.rows {
            *by_kind.entry(r.kind).or_default() += 1;
            *matrix
                .entry((r.detector.clone(), r.kind, row_category(&d.sites, r, ix)))
                .or_default() += 1;
        }
    }
    let _ = writeln!(md, "## Summary\n");
    let _ = writeln!(
        md,
        "- workspaces: {} ({} failed to build)",
        corpora.len() + errors.len(),
        errors.len()
    );
    let _ = writeln!(md, "- findings: old {fo}, new {fnew}");
    let _ = writeln!(md, "- detector diagnostics: old {dold}, new {dnew}");
    let _ = writeln!(
        md,
        "- differences: removed {}, added {}, changed {}",
        by_kind.get(&DiffKind::Removed).unwrap_or(&0),
        by_kind.get(&DiffKind::Added).unwrap_or(&0),
        by_kind.get(&DiffKind::Changed).unwrap_or(&0)
    );
    let _ = writeln!(md, "- differing call sites / record ops: {nsites}\n");
    if !matrix.is_empty() {
        let _ = writeln!(md, "| detector | kind | category | count |");
        let _ = writeln!(md, "|---|---|---|---:|");
        for ((det, kind, cat), n) in &matrix {
            let _ = writeln!(md, "| {det} | {} | {} | {n} |", kind.as_str(), cell(cat));
        }
        let _ = writeln!(md);
    }
    if !site_cats.is_empty() {
        let _ = writeln!(md, "Differing sites by category:\n");
        let _ = writeln!(md, "| category | sites |");
        let _ = writeln!(md, "|---|---:|");
        for (cat, n) in &site_cats {
            let _ = writeln!(md, "| {} | {n} |", cell(cat));
        }
        let _ = writeln!(md);
    }

    // Differences, grouped by attributed category.
    let mut grouped: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, d) in corpora {
        for (r, ix) in &d.rows {
            // ponytail: first 12 sites per row; the full list is in "Differing call sites".
            let mut attributed: Vec<String> = ix
                .iter()
                .take(12)
                .map(|&i| {
                    let s = &d.sites[i];
                    format!(
                        "{} `{}` [{}]",
                        pre(name, &s.location),
                        s.callee,
                        s.categories.join(", ")
                    )
                })
                .collect();
            if ix.len() > 12 {
                attributed.push(format!("(+{} more)", ix.len() - 12));
            }
            grouped
                .entry(row_category(&d.sites, r, ix))
                .or_default()
                .push(format!(
                    "| {} | {} | {} | {} | {} | {} | |",
                    r.detector,
                    cell(&pre(name, &r.location)),
                    cell(&r.root_cause_key),
                    r.kind.as_str(),
                    cell(&r.what),
                    cell(&attributed.join("<br>"))
                ));
        }
    }
    let _ = writeln!(md, "## Differences\n");
    if grouped.is_empty() {
        let _ = writeln!(md, "None.\n");
    }
    for (cat, rows) in &grouped {
        let _ = writeln!(md, "### {cat} ({})\n", rows.len());
        let _ = writeln!(
            md,
            "| detector | location | root cause key | kind | what changed | attributed sites + category | verdict |"
        );
        let _ = writeln!(md, "|---|---|---|---|---|---|---|");
        for row in rows {
            let _ = writeln!(md, "{row}");
        }
        let _ = writeln!(md);
    }

    // Every differing site.
    let _ = writeln!(md, "## Differing call sites\n");
    if nsites == 0 {
        let _ = writeln!(md, "None.\n");
    } else {
        let _ = writeln!(
            md,
            "| location | callee | old ({old_name}) | new ({new_name}) | program evidence | category |"
        );
        let _ = writeln!(md, "|---|---|---|---|---|---|");
        for (name, d) in corpora {
            for s in &d.sites {
                let _ = writeln!(
                    md,
                    "| {} | `{}` | {} | {} | {} | {} |",
                    cell(&pre(name, &s.location)),
                    cell(&s.callee),
                    cell(&s.old),
                    cell(&s.new),
                    cell(&s.program),
                    cell(&s.categories.join(", "))
                );
            }
        }
        let _ = writeln!(md);
    }

    if !errors.is_empty() {
        let _ = writeln!(md, "## Workspaces that failed to build\n");
        for (name, e) in errors {
            let _ = writeln!(md, "- {name}: {}", cell(e));
        }
        let _ = writeln!(md);
    }

    if let [(_, d)] = corpora {
        let mut v = serde_json::to_value(&d.census).unwrap_or_default();
        if let Some(o) = v.as_object_mut() {
            let n = o
                .remove("unmatched")
                .and_then(|u| u.as_array().map(Vec::len));
            o.insert("unmatched_count".to_string(), n.unwrap_or(0).into());
        }
        let _ = writeln!(md, "## Adapter census\n");
        let _ = writeln!(
            md,
            "```json\n{}\n```",
            serde_json::to_string_pretty(&v).unwrap_or_default()
        );
    }
    md
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::l5::finding::{EvidenceStep, FindingConfidence, SourceAnchor};

    fn anchor(line: u32, routine: &str) -> SourceAnchor {
        SourceAnchor {
            source_unit_id: "ws:src/a.al".to_string(),
            start_line: line,
            start_column: 4,
            end_line: line,
            end_column: 10,
            enclosing_routine_id: routine.to_string(),
            syntax_kind: "call".to_string(),
            normalized_text_hash: None,
            leading_context_hash: None,
            trailing_context_hash: None,
        }
    }

    fn finding(detector: &str, line: u32, rck: &str, severity: &str) -> Finding {
        Finding {
            id: format!("{detector}-{line}"),
            root_cause_key: rck.to_string(),
            detector: detector.to_string(),
            title: Arc::from("T"),
            root_cause: "rc".to_string(),
            severity: severity.to_string(),
            confidence: FindingConfidence {
                level: "high".to_string(),
                capped_by: None,
                evidence: Vec::new(),
            },
            primary_location: anchor(line, "r1"),
            evidence_path: vec![EvidenceStep {
                routine_id: "r2".to_string(),
                operation_id: None,
                callsite_id: None,
                loop_id: None,
                source_anchor: anchor(line, "r2"),
                note: "n".to_string(),
            }],
            additional_paths: None,
            affected_objects: Vec::new(),
            affected_tables: Vec::new(),
            fix_options: Vec::new(),
            provenance: Vec::new(),
            actionable_anchor: None,
            fingerprint: None,
            event_kind: None,
            cross_extension_subscribers: None,
            cohort_contexts: None,
        }
    }

    #[test]
    fn identical_sets_have_no_difference() {
        let a = vec![finding("d1", 1, "k", "high"), finding("d2", 2, "k", "low")];
        assert!(diff_findings(&a, &a.clone()).is_empty());
    }

    #[test]
    fn a_finding_only_in_old_is_removed() {
        let old = vec![finding("d1", 1, "k", "high"), finding("d2", 2, "k", "low")];
        let new = vec![finding("d2", 2, "k", "low")];
        let d = diff_findings(&old, &new);
        assert_eq!(d.len(), 1, "{d:#?}");
        assert_eq!(d[0].kind, DiffKind::Removed);
        assert_eq!(d[0].detector, "d1");
        assert_eq!(d[0].location, "src/a.al:2:5");
        assert_eq!(d[0].routines, vec!["r1", "r2"]);
    }

    #[test]
    fn a_finding_only_in_new_is_added_and_the_key_includes_the_root_cause() {
        let old = vec![finding("d1", 1, "k", "high")];
        let new = vec![
            finding("d1", 1, "k", "high"),
            finding("d1", 1, "k2", "high"),
        ];
        let d = diff_findings(&old, &new);
        assert_eq!(d.len(), 1, "{d:#?}");
        assert_eq!(d[0].kind, DiffKind::Added);
        assert_eq!(d[0].root_cause_key, "k2");
    }

    #[test]
    fn same_key_with_other_severity_cap_or_path_is_changed() {
        let old = vec![finding("d1", 1, "k", "high")];
        let mut n = finding("d1", 1, "k", "medium");
        n.confidence.capped_by = Some(vec!["unresolved-call".to_string()]);
        n.evidence_path.clear();
        n.root_cause = "rc2".to_string();
        let d = diff_findings(&old, &[n]);
        assert_eq!(d.len(), 1, "{d:#?}");
        assert_eq!(d[0].kind, DiffKind::Changed);
        assert_eq!(
            d[0].what,
            "severity high -> medium; cappedBy [] -> [\"unresolved-call\"]; \
             evidence path (1 -> 0 steps), first difference at step 1: src/a.al:2:5 n -> (none); \
             rootCause \"rc\" -> \"rc2\""
        );
    }

    #[test]
    fn attribution_takes_sites_in_or_into_the_findings_routines_only() {
        let site = |routine: &str, target: &str| SiteDiff {
            routine: routine.to_string(),
            targets: vec![target.to_string()],
            location: "src/a.al:1:1".to_string(),
            callee: "X".to_string(),
            old: "o".to_string(),
            new: "n".to_string(),
            program: "-".to_string(),
            categories: vec!["external-record-receiver".to_string()],
        };
        // The finding's routines are r1 (primary) and r2 (evidence path).
        // r9 → r7 is unrelated; r8 → r1 is a caller of the finding's routine.
        let sites = vec![
            site("r9", "r7"),
            site("r2", "r7"),
            site("r1", "r7"),
            site("r8", "r1"),
        ];
        let diffs = || diff_findings(&[finding("d1", 1, "k", "high")], &[]);
        let edge = |f: &str, t: &str| (f.to_string(), t.to_string());
        let rows = attribute(diffs(), &sites, &[]);
        assert_eq!(rows[0].1, vec![1, 2, 3]);
        assert_eq!(rows[0].0.via, None);
        assert_eq!(
            row_category(&sites, &rows[0].0, &rows[0].1),
            "external-record-receiver"
        );
        assert_eq!(row_category(&sites, &rows[0].0, &[]), "unattributed");

        // Nothing in or into r1/r2; r9 → r5 differs and r5 calls r1, so
        // the r9 site is attributed through the callers.
        let sites = vec![site("r9", "r5"), site("r9", "r6")];
        let rows = attribute(diffs(), &sites, &[edge("r5", "r1")]);
        assert_eq!(rows[0].1, vec![0]);
        assert_eq!(
            row_category(&sites, &rows[0].0, &rows[0].1),
            "via callers: external-record-receiver"
        );
        assert!(attribute(diffs(), &sites, &[])[0].1.is_empty());

        // No caller path; r2 → r4 → r9 reaches the differing r9 sites.
        let rows = attribute(diffs(), &sites, &[edge("r2", "r4"), edge("r4", "r9")]);
        assert_eq!(rows[0].1, vec![0, 1]);
        assert_eq!(rows[0].0.via, Some("callees"));
    }

    const OLD: &str = "# T\n\n\
        | detector | kind | category | count |\n\
        |---|---|---|---:|\n\
        | d1 | removed | a \\| b | 2 |\n\n\
        | detector | location | root cause key | kind | what changed | attributed sites + category | verdict |\n\
        |---|---|---|---|---|---|---|\n\
        | d1 | a.al:1:1 | k1 | removed | x \\| y | s | fixed |\n\
        | d1 | a.al:2:1 | k2 | added | x | s | regression |\n";

    #[test]
    fn verdicts_survive_a_regen_and_follow_their_row() {
        // Row k1 is unchanged, k2 is gone, k3 is new.
        let new = OLD.replace("| fixed |", "| |").replace(
            "| d1 | a.al:2:1 | k2 | added | x | s | regression |",
            "| d1 | a.al:3:1 | k3 | added | x | s | |",
        );
        let carried = carry_verdicts(&new, OLD);
        assert!(
            carried.contains("| d1 | a.al:1:1 | k1 | removed | x \\| y | s | fixed |"),
            "{carried}"
        );
        assert!(
            carried.contains("| d1 | a.al:3:1 | k3 | added | x | s | |"),
            "{carried}"
        );
        assert!(!carried.contains("regression"), "{carried}");
        // Everything else is byte-identical to the new text.
        assert_eq!(strip_verdicts(&carried), strip_verdicts(&new));
        assert_eq!(carry_verdicts(&new, ""), new);
    }

    #[test]
    fn strip_verdicts_touches_only_difference_rows() {
        let s = strip_verdicts(OLD);
        // Summary rows keep their count; escaped pipes are not separators.
        assert!(s.contains("| d1 | removed | a \\| b | 2 |"), "{s}");
        assert!(
            s.contains("| d1 | a.al:1:1 | k1 | removed | x \\| y | s \n"),
            "{s}"
        );
        assert!(!s.contains("fixed"), "{s}");
        // A summary count change is visible after stripping.
        assert_ne!(
            s,
            strip_verdicts(&OLD.replace("a \\| b | 2 |", "a \\| b | 3 |"))
        );
    }

    #[test]
    fn text_change_shows_the_first_differing_span() {
        assert_eq!(
            text_change("Commit reached by 3 loops", "Commit reached by 5 loops"),
            "\"Commit reached by 3…\" -> \"Commit reached by 5…\""
        );
        assert_eq!(text_change("ab", "abc"), "\"ab\" -> \"abc\"");
    }
}
