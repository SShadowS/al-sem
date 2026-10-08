//! The semantic-edges golden minted from the AL compiler's call graph
//! (engine-switch S9.0d; owner decision R1).
//!
//! The AL extension's `altool graph extract-whole` compiles the workspace and its
//! dependency sources together and writes the compiler's own call graph
//! ([`compiler_oracle`]). Its granularity is the (caller, callee) PAIR per edge
//! class, at the first line it occurs: no column, no callee text. So the golden is
//! a set of pairs, not of call sites. It replaces the three L3-minted per-site
//! goldens (`cdo-anon.json`, `cdo-trigger-anon.json`, `cdo-event-anon.json`).
//!
//! **The golden.** `cdo-compiler-anon.json` holds every pair whose caller is in
//! the workspace app, anonymized under [`anon::PAIR_DOMAIN_V1`] (caller and
//! target are each one id over app, object and routine; the edge class, the
//! caller's object kind and the line stay cleartext). It is stamped with the
//! workspace's git and dependency-closure state ([`MintMetadata`]) and the AL
//! extension the graph came from ([`CompilerStamp`]).
//!
//! **The audit** ([`run_compiler_audit`]) projects the program resolver's
//! workspace edges to the same pairs and compares. Both engines are right about
//! some things the other is not, so every disagreeing pair must be EXPLAINED by
//! one [`Verdict`], each a rule over facts the program holds at that pair; a pair
//! no rule explains is a failure. The rules are the shapes the S9.0b/c triage
//! established on CDO (`docs/s9-oracle/cdo-workspace-triage.md`), each a compiler
//! limit or a deliberate policy difference, never a program bug: a program bug is
//! fixed, not explained.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::program::node::RoutineNodeId;
use crate::program::resolve::anon::{self, AnonId};
use crate::program::resolve::compiler_oracle::{
    CompilerGraph, EdgeClass, Routine, Target, pair_lines, program_sites, reclassify_entry_runs,
    routine_of_node, target_of_node,
};
use crate::program::resolve::differential::project_fresh;
use crate::program::resolve::edge::{EdgeKind, RouteTarget};
use crate::program::resolve::full::{ProgramContext, ProgramReport, SiteFacts};
use crate::program::resolve::semantic_golden::{
    DriftHandler, MintMetadata, merge_deanon_map, workspace_drift,
};

/// Schema of `cdo-compiler-anon.json`. Bump on a field-shape or anonymization
/// change.
pub const COMPILER_GOLDEN_SCHEMA_VERSION: u32 = 1;

/// The AL extension whose `altool` produced the graph, e.g.
/// `ms-dynamics-smb.al-18.0.2732683`: a new compiler can move the golden.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompilerStamp {
    pub extension: String,
}

/// One compiler pair, anonymized.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AnonPair {
    pub caller: AnonId,
    /// The caller's object kind (`codeunit`, `page`, ...): cleartext.
    pub caller_kind: String,
    pub class: EdgeClass,
    pub target: AnonId,
    /// 1-based line of the pair's first occurrence in the caller, for reading
    /// only (the audit compares pairs, not lines). 0 when the compiler graph
    /// records no line: its `Interface` and `Event` edges carry none.
    pub line: u32,
}

/// The committed golden.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CompilerGolden {
    pub schema_version: u32,
    pub metadata: MintMetadata,
    pub compiler: CompilerStamp,
    /// Sorted.
    pub pairs: Vec<AnonPair>,
}

/// Path of the committed CDO golden.
#[must_use]
pub fn cdo_compiler_golden_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/goldens/semantic-edges/cdo-compiler-anon.json")
}

/// Path of the committed in-repo fixture golden (`tests/fixtures/semantic-golden`).
#[must_use]
pub fn fixture_compiler_golden_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/goldens/semantic-edges/fixture-compiler-anon.json")
}

/// The workspace the fixture golden is minted from.
#[must_use]
pub fn fixture_workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/semantic-golden")
}

/// Load a golden; `None` when missing or unreadable.
#[must_use]
pub fn load_compiler_golden(path: &Path) -> Option<CompilerGolden> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn anon_caller(r: &Routine) -> AnonId {
    anon::anon(
        anon::PAIR_DOMAIN_V1,
        &format!(
            "caller\u{1}{}\u{1}{}\u{1}{}\u{1}{}",
            r.app, r.object_kind, r.object_key, r.routine
        ),
    )
}

fn anon_target(t: &Target) -> AnonId {
    anon::anon(
        anon::PAIR_DOMAIN_V1,
        &format!(
            "target\u{1}{}\u{1}{}\u{1}{}",
            t.app, t.object_key, t.routine
        ),
    )
}

fn deanon_caller(r: &Routine, map: &mut BTreeMap<String, String>) -> AnonId {
    let id = anon_caller(r);
    map.insert(
        id.0.clone(),
        format!("{} {} {}.{}", r.app, r.object_kind, r.object_key, r.routine),
    );
    id
}

fn deanon_target(t: &Target, map: &mut BTreeMap<String, String>) -> AnonId {
    let id = anon_target(t);
    map.insert(
        id.0.clone(),
        format!("{} {}.{}", t.app, t.object_key, t.routine),
    );
    id
}

/// Mint the golden from a compiler graph: every pair whose caller is in the app
/// `workspace_guid` (lower-case). Also returns the plaintext of every id, for the
/// gitignored local de-anonymization map.
#[must_use]
pub fn mint_compiler_golden(
    graph: &CompilerGraph,
    workspace_guid: &str,
    metadata: MintMetadata,
    compiler: CompilerStamp,
) -> (CompilerGolden, BTreeMap<String, String>) {
    let mut deanon = BTreeMap::new();
    let mut pairs: Vec<AnonPair> = pair_lines(&graph.sites)
        .into_iter()
        .filter(|((caller, _, _), _)| caller.app == workspace_guid)
        .map(|((caller, class, target), line)| AnonPair {
            caller: deanon_caller(&caller, &mut deanon),
            caller_kind: caller.object_kind.clone(),
            class,
            target: deanon_target(&target, &mut deanon),
            line,
        })
        .collect();
    pairs.sort();
    (
        CompilerGolden {
            schema_version: COMPILER_GOLDEN_SCHEMA_VERSION,
            metadata,
            compiler,
            pairs,
        },
        deanon,
    )
}

/// Why a disagreeing pair is not a defect. Each is a rule over the program's own
/// facts at the pair (see [`classify`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum Verdict {
    /// Compiler-only `Call` from a subscriber to its publisher: the compiler
    /// records the event name in `[EventSubscriber(..)]` as a direct call (S4).
    /// Rule: the program has an `EventFlow` edge from that publisher to that
    /// subscriber.
    SubscriberAttribute,
    /// Compiler-only `Trigger`: the compiler's trigger edges ignore RunTrigger
    /// (S9). Rule: one of the caller's record operations excluded that trigger
    /// because its RunTrigger is false ([`SiteFacts::run_trigger_excluded`]).
    RunTriggerFalse,
    /// Program-only `Trigger` into a field's `OnValidate`: the compiler graph has
    /// no `Validate` trigger edges (S12). Rule: the target is a field-level
    /// `OnValidate`.
    ValidateTrigger,
    /// Program-only `Trigger` from a record operation written without a receiver
    /// (`Insert(true)` on the implicit `Rec`): the compiler graph has no trigger
    /// edges from one (S11). Rule: every program site of the pair is one
    /// ([`SiteFacts::unqualified_record_op`]).
    UnqualifiedRecordOp,
    /// Program-only `Event` from a platform page event (`OnOpenPageEvent`): the
    /// compiler graph has no publisher for platform trigger events (S13). Rule:
    /// the publisher is a synthetic `PublisherKind::Platform` routine on a page.
    PlatformPageEvent,
    /// Program-only `Call` from code under an `#if` arm the workspace does not
    /// build: the program reads every arm (a superset of all builds), the
    /// compiler only the defined one (S7). Rule: every program site's build
    /// context ([`SiteFacts::build_context`]) contradicts the symbols `app.json`
    /// defines (`preprocessorSymbols`; case-sensitive).
    InactivePreprocArm,
    /// Program-only `Run`: the compiler graph has no object-run edges.
    ObjectRun,
}

/// Which engine reports a disagreeing pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum Side {
    CompilerOnly,
    ProgramOnly,
}

/// The audit's result.
#[derive(Clone, Debug, Default, Serialize)]
pub struct CompilerAuditReport {
    pub golden_loaded: bool,
    pub compiler_pairs: usize,
    pub program_pairs: usize,
    pub agree: usize,
    /// Disagreeing pairs per (side, class, verdict); `None` = unexplained.
    pub disagreements: BTreeMap<(Side, EdgeClass, Option<Verdict>), usize>,
    /// The unexplained pairs, readable when the local de-anon map has them.
    pub unexplained: Vec<String>,
    /// SHA-256 over the sorted classified pairs (determinism check).
    pub digest: String,
}

/// One program pair: its first line and the edges that make it.
struct ProgramPair {
    line: u32,
    edges: Vec<usize>,
    caller: Routine,
    target: Target,
}

/// The program resolver's workspace-caller edges as compiler-shaped pairs.
fn program_pairs(
    ctx: &ProgramContext,
    report: &ProgramReport,
) -> BTreeMap<(AnonId, EdgeClass, AnonId), ProgramPair> {
    let apps = &ctx.graph().apps;
    let ws = report.primary_app_ref;
    let idx: Vec<usize> = (0..report.edges.len())
        .filter(|&i| report.edges[i].edge.site.caller.object.app == ws)
        .collect();
    let mut edges: Vec<_> = idx.iter().map(|&i| report.edges[i].edge.clone()).collect();
    reclassify_entry_runs(&mut edges, ctx.graph());
    let mut out: BTreeMap<(AnonId, EdgeClass, AnonId), ProgramPair> = BTreeMap::new();
    for (k, e) in edges.iter().enumerate() {
        let sites = program_sites(&project_fresh(std::slice::from_ref(e), apps));
        for ((caller, class, target), line) in pair_lines(&sites) {
            let key = (anon_caller(&caller), class, anon_target(&target));
            let p = out.entry(key).or_insert(ProgramPair {
                line,
                edges: Vec::new(),
                caller,
                target,
            });
            p.line = p.line.min(line);
            p.edges.push(idx[k]);
        }
    }
    out
}

/// Preprocessor symbols `app.json` defines (`preprocessorSymbols`); empty when
/// absent or unreadable.
fn defined_symbols(workspace_root: &Path) -> HashSet<String> {
    std::fs::read_to_string(workspace_root.join("app.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| {
            v.get("preprocessorSymbols")?.as_array().map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(str::to_string))
                    .collect()
            })
        })
        .unwrap_or_default()
}

/// Facts the compiler-only rules read, as anonymized (caller, target) pairs.
struct CompilerOnlyFacts {
    /// (subscriber, publisher) of every program `EventFlow` route.
    subscriptions: HashSet<(AnonId, AnonId)>,
    /// (caller, trigger) each RunTrigger-false record operation excluded.
    run_trigger_false: HashSet<(AnonId, AnonId)>,
}

fn compiler_only_facts(ctx: &ProgramContext, report: &ProgramReport) -> CompilerOnlyFacts {
    let apps = &ctx.graph().apps;
    let mut subscriptions = HashSet::new();
    let mut run_trigger_false = HashSet::new();
    for ce in &report.edges {
        if ce.edge.kind == EdgeKind::EventFlow {
            let publisher = anon_target(&target_of_node(&ce.edge.from, apps));
            for r in &ce.edge.routes {
                if let RouteTarget::Routine(sub) = &r.target {
                    subscriptions
                        .insert((anon_caller(&routine_of_node(sub, apps)), publisher.clone()));
                }
            }
        }
        if let Some(f) = report.site_facts.get(&ce.obligation_id) {
            let caller = anon_caller(&routine_of_node(&ce.edge.site.caller, apps));
            for t in &f.run_trigger_excluded {
                run_trigger_false.insert((caller.clone(), anon_target(&target_of_node(t, apps))));
            }
        }
    }
    CompilerOnlyFacts {
        subscriptions,
        run_trigger_false,
    }
}

/// The verdict for a program-only pair, or `None`.
fn classify_program_only(
    class: EdgeClass,
    pair: &ProgramPair,
    ctx: &ProgramContext,
    report: &ProgramReport,
    defined: &HashSet<String>,
) -> Option<Verdict> {
    let apps = &ctx.graph().apps;
    let facts = |i: usize| -> SiteFacts {
        report
            .site_facts
            .get(&report.edges[i].obligation_id)
            .cloned()
            .unwrap_or_default()
    };
    let all_sites = |p: &dyn Fn(&SiteFacts) -> bool| pair.edges.iter().all(|&i| p(&facts(i)));
    // The pair's target as a program routine id.
    let target_id = || -> Option<&RoutineNodeId> {
        pair.edges.iter().find_map(|&i| {
            report.edges[i]
                .edge
                .routes
                .iter()
                .find_map(|r| match &r.target {
                    RouteTarget::Routine(id) if target_of_node(id, apps) == pair.target => Some(id),
                    _ => None,
                })
        })
    };
    match class {
        EdgeClass::Run => Some(Verdict::ObjectRun),
        EdgeClass::Trigger => {
            if target_id()
                .is_some_and(|t| t.name_lc == "onvalidate" && t.enclosing_member_lc.is_some())
            {
                Some(Verdict::ValidateTrigger)
            } else if all_sites(&|f| f.unqualified_record_op) {
                Some(Verdict::UnqualifiedRecordOp)
            } else {
                None
            }
        }
        EdgeClass::Event => {
            let publisher = &report.edges[*pair.edges.first()?].edge.from;
            let platform_page = publisher.object.kind == al_syntax::ir::ObjectKind::Page
                && ctx
                    .graph()
                    .routines
                    .run_by(|r| r.id.cmp(publisher))
                    .any(|r| {
                        r.publisher_kind
                            == Some(crate::program::resolve::event::PublisherKind::Platform)
                    });
            platform_page.then_some(Verdict::PlatformPageEvent)
        }
        EdgeClass::Call => {
            let outside_build = |f: &SiteFacts| {
                !f.build_context.is_empty()
                    && f.build_context
                        .iter()
                        .any(|(s, v)| *v != defined.contains(s))
            };
            all_sites(&outside_build).then_some(Verdict::InactivePreprocArm)
        }
    }
}

/// Hand `golden`'s drift from `workspace_root` (git state or dependency closure
/// changed since the mint) to `on_drift`; what drift means is the caller's call.
pub fn check_drift(golden: &CompilerGolden, workspace_root: &Path, on_drift: DriftHandler) {
    if let Some(msg) = workspace_drift(&golden.metadata, workspace_root) {
        on_drift(&msg);
    }
}

/// Audit the program resolver against a compiler golden. `report` is the
/// workspace report `ctx` produced. `deanon_map`, when given, receives the
/// plaintext of every program pair the audit saw (the gitignored local map that
/// makes a failing anonymized pair readable).
#[must_use]
pub fn run_compiler_audit(
    ctx: &ProgramContext,
    report: &ProgramReport,
    workspace_root: &Path,
    golden: Option<&CompilerGolden>,
    on_drift: DriftHandler,
    deanon_map: Option<&Path>,
) -> CompilerAuditReport {
    let Some(golden) = golden else {
        return CompilerAuditReport::default();
    };
    check_drift(golden, workspace_root, on_drift);
    let program = program_pairs(ctx, report);
    let compiler: HashMap<(AnonId, EdgeClass, AnonId), &AnonPair> = golden
        .pairs
        .iter()
        .map(|p| ((p.caller.clone(), p.class, p.target.clone()), p))
        .collect();
    let facts = compiler_only_facts(ctx, report);
    let defined = defined_symbols(workspace_root);

    let mut r = CompilerAuditReport {
        golden_loaded: true,
        compiler_pairs: compiler.len(),
        program_pairs: program.len(),
        ..Default::default()
    };
    let mut deanon = BTreeMap::new();
    let mut classified: BTreeSet<(Side, EdgeClass, Option<Verdict>, AnonId, AnonId)> =
        BTreeSet::new();
    for (key, pair) in &program {
        deanon_caller(&pair.caller, &mut deanon);
        deanon_target(&pair.target, &mut deanon);
        if compiler.contains_key(key) {
            r.agree += 1;
            continue;
        }
        let v = classify_program_only(key.1, pair, ctx, report, &defined);
        if v.is_none() {
            r.unexplained.push(format!(
                "program-only {:?} {:?} -> {:?} line {}",
                key.1, pair.caller, pair.target, pair.line
            ));
        }
        classified.insert((Side::ProgramOnly, key.1, v, key.0.clone(), key.2.clone()));
    }
    for (key, p) in &compiler {
        if program.contains_key(key) {
            continue;
        }
        let pair = (key.0.clone(), key.2.clone());
        let v = match key.1 {
            EdgeClass::Call if facts.subscriptions.contains(&pair) => {
                Some(Verdict::SubscriberAttribute)
            }
            EdgeClass::Trigger if facts.run_trigger_false.contains(&pair) => {
                Some(Verdict::RunTriggerFalse)
            }
            _ => None,
        };
        if v.is_none() {
            r.unexplained.push(format!(
                "compiler-only {:?} {} {} -> {} line {}",
                key.1, p.caller_kind, p.caller.0, p.target.0, p.line
            ));
        }
        classified.insert((Side::CompilerOnly, key.1, v, key.0.clone(), key.2.clone()));
    }
    if let Some(path) = deanon_map {
        merge_deanon_map(path, &deanon);
    }
    let mut h = Sha256::new();
    for (side, class, v, c, t) in &classified {
        *r.disagreements.entry((*side, *class, *v)).or_default() += 1;
        h.update(format!("{side:?}|{class:?}|{v:?}|{}|{}\n", c.0, t.0).as_bytes());
    }
    h.update(format!("agree={}", r.agree).as_bytes());
    r.digest = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    r.unexplained.sort();
    r
}
