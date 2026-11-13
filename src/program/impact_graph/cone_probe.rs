//! `aldump --reverse-cone` (agent-tasks design §3.8): the measurement probe
//! of the reverse walker. It builds a workspace's cross-app program and its
//! sited graph, then walks from each named root ([`walk_one`], one scratch
//! reused) or from all of them at once ([`walk_multi`]), and prints what the
//! walks found and cost, one JSON object. Not a user-facing tool: C, A and B1
//! build theirs on the same walker.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::time::Instant;

use serde_json::{Value, json};

use super::reverse::{
    ConeVisitor, EntryKind, ExternalWhy, HiddenChannel, NodeIx, Tier, Via, WalkOptions,
    WalkScratch, WalkState, walk_multi, walk_one,
};
use super::sited::{SitedGraph, flag};

/// What the probe is asked.
pub struct ProbeArgs {
    /// Roots as `Object Name::Routine` (case-insensitive) or StableRoutineIds.
    pub roots: Vec<String>,
    /// Walk all roots in one `walk_multi` instead of one `walk_one` each.
    pub multi: bool,
    pub assume_no_dependents: bool,
}

/// Per-root (or per-walk) aggregate of one cone.
#[derive(Default)]
struct Cone {
    states: usize,
    /// Best `(tier, hops)` per node.
    best: HashMap<NodeIx, (Tier, u32)>,
    hidden: BTreeSet<NodeIx>,
    universal: bool,
    entries: BTreeMap<EntryKind, BTreeSet<NodeIx>>,
    external: BTreeMap<&'static str, BTreeSet<NodeIx>>,
    /// Parent of each node's best state, for one witness path.
    parent: HashMap<NodeIx, NodeIx>,
}

/// The probe's visitor: one [`Cone`] per root label.
struct Probe {
    cones: Vec<Cone>,
}

impl ConeVisitor for Probe {
    fn reach(&mut self, root: u32, s: WalkState, hops: u32, _: Via, from: Option<WalkState>) {
        let c = &mut self.cones[root as usize];
        c.states += 1;
        let better = c
            .best
            .get(&s.node)
            .is_none_or(|b| (s.tier, hops) < (b.0, b.1));
        if better {
            c.best.insert(s.node, (s.tier, hops));
            if let Some(f) = from {
                c.parent.insert(s.node, f.node);
            }
        }
    }
    fn hidden(&mut self, root: u32, n: NodeIx, ch: HiddenChannel) {
        let c = &mut self.cones[root as usize];
        match ch {
            HiddenChannel::Universal => c.universal = true,
            HiddenChannel::Flag(_) => {
                c.hidden.insert(n);
            }
        }
    }
    fn entry(&mut self, root: u32, n: NodeIx, kinds: &[EntryKind], _: WalkState, _: u32) {
        for k in kinds {
            self.cones[root as usize]
                .entries
                .entry(*k)
                .or_default()
                .insert(n);
        }
    }
    fn external(&mut self, root: u32, n: NodeIx, why: ExternalWhy, _: WalkState, _: u32) {
        self.cones[root as usize]
            .external
            .entry(why.as_str())
            .or_default()
            .insert(n);
    }
}

fn status_kib(key: &str) -> Option<u64> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = s.lines().find(|l| l.starts_with(key))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

/// One cone's summary (`object_name` names a node's object).
fn summarize(
    g: &SitedGraph,
    root: NodeIx,
    c: &Cone,
    object_name: &dyn Fn(NodeIx) -> String,
) -> Value {
    let tests: Vec<(NodeIx, Tier, u32)> = c
        .best
        .iter()
        .filter(|(n, _)| g.flags(**n) & flag::TEST != 0)
        .map(|(n, (t, h))| (*n, *t, *h))
        .collect();
    let count = |max: Tier| tests.iter().filter(|t| t.1 <= max).count();
    let local: Vec<&(NodeIx, Tier, u32)> = tests.iter().filter(|t| t.1 == Tier::Local).collect();
    let local_codeunits: BTreeSet<String> = local.iter().map(|t| object_name(t.0)).collect();
    let local_hops = (
        local.iter().map(|t| t.2).min(),
        local.iter().map(|t| t.2).max(),
    );
    let nearest = tests.iter().min_by_key(|t| (t.1, t.2, t.0)).map(|t| {
        let mut path = vec![g.display(t.0)];
        let mut at = t.0;
        while let Some(p) = c.parent.get(&at) {
            path.push(g.display(*p));
            at = *p;
            if path.len() > 64 {
                break;
            }
        }
        json!({"test": g.display(t.0), "codeunit": object_name(t.0),
                   "tier": t.1.as_str(), "hops": t.2, "path": path})
    });
    let resolved_hidden = c
        .hidden
        .iter()
        .filter(|n| {
            c.best
                .get(n)
                .is_some_and(|b| b.0 <= Tier::ThroughDependency)
        })
        .count();
    let direct_callers: BTreeSet<NodeIx> = g
        .sites_into(root)
        .iter()
        .map(|s| g.sites()[*s as usize].from)
        .collect();
    json!({
        "root": g.display(root),
        "stableId": g.stable_id(root),
        "states": c.states,
        "nodes": c.best.len(),
        "nodesByTier": {
            "local": c.best.values().filter(|b| b.0 == Tier::Local).count(),
            "throughDependency": c.best.values().filter(|b| b.0 == Tier::ThroughDependency).count(),
            "onlyOpen": c.best.values().filter(|b| b.0 == Tier::OnlyOpen).count(),
        },
        "tests": {"local": count(Tier::Local), "resolved": count(Tier::ThroughDependency),
                  "any": count(Tier::OnlyOpen),
                  "localCodeunits": local_codeunits.len(),
                  "localHops": [local_hops.0, local_hops.1],
                  "nearest": nearest},
        "hiddenNodes": {"resolved": resolved_hidden, "any": c.hidden.len(), "universal": c.universal,
                        "resolvedIds": c.hidden.iter()
                            .filter(|n| c.best.get(n).is_some_and(|b| b.0 <= Tier::ThroughDependency))
                            .map(|n| g.node_id(*n)).collect::<Vec<_>>()},
        "entries": c.entries.iter().map(|(k, v)| (k.as_str().to_string(), json!(v.len()))).collect::<serde_json::Map<_, _>>(),
        "external": c.external.iter().map(|(k, v)| ((*k).to_string(), json!(v.len()))).collect::<serde_json::Map<_, _>>(),
        "directCallerSites": g.sites_into(root).len(),
        "directCallers": direct_callers.len(),
    })
}

/// Run the probe over `ws` (see the module doc).
pub fn run(ws: &Path, args: &ProbeArgs) -> Result<Value, String> {
    let t0 = Instant::now();
    let inputs = crate::program::inputs::WorkspaceInputs::read(ws, &Default::default());
    let program = crate::program::resolved::ResolvedProgram::build(
        &inputs,
        crate::program::resolved::ProgramScope::CrossApp,
    )
    .map_err(|e| format!("the program did not build: {e:?}"))?;
    let build_ms = t0.elapsed().as_millis() as u64;
    let t1 = Instant::now();
    let g = super::sited_from_program(&program, &|| false).map_err(|_| "cancelled")?;
    let sited_ms = t1.elapsed().as_millis() as u64;
    let graph = program.body().ctx.graph();
    let object_name = |n: NodeIx| -> String {
        g.routine_object(n)
            .and_then(|o| {
                graph
                    .objects
                    .binary_search_by(|x| x.id.cmp(&o))
                    .ok()
                    .map(|i| graph.objects[i].name.to_string())
            })
            .unwrap_or_default()
    };
    let mut roots: Vec<NodeIx> = Vec::new();
    let mut unresolved: Vec<String> = Vec::new();
    for r in &args.roots {
        let found: Vec<NodeIx> = if r.contains('#') {
            g.nodes_with_stable_id(r)
        } else {
            let (obj, name) = r.rsplit_once("::").unwrap_or(("", r));
            g.routines_named(&al_syntax::fold_identifier(name))
                .into_iter()
                .filter(|n| obj.is_empty() || object_name(*n).eq_ignore_ascii_case(obj))
                .collect()
        };
        match found.first() {
            Some(n) => roots.push(*n),
            None => unresolved.push(r.clone()),
        }
    }
    let opts = WalkOptions {
        assume_no_dependents: args.assume_no_dependents,
        ..Default::default()
    };
    let rss_before = status_kib("VmRSS:");
    let mut scratch = WalkScratch::default();
    let mut out = json!({
        "workspace": ws.display().to_string(),
        "buildMs": build_ms,
        "sited": {"ms": sited_ms, "nodes": g.node_count(), "sites": g.site_count(), "bytes": g.bytes()},
        "roots": roots.len(),
        "unresolvedRoots": unresolved,
    });
    if args.multi {
        let mut probe = Probe {
            cones: roots.iter().map(|_| Cone::default()).collect(),
        };
        let t = Instant::now();
        let stats = walk_multi(&g, &roots, &opts, &mut scratch, &mut probe);
        let walk_ms = t.elapsed().as_secs_f64() * 1000.0;
        // The walk alone, with a visitor that keeps nothing.
        struct Nothing;
        impl ConeVisitor for Nothing {}
        let t = Instant::now();
        let bare = walk_multi(&g, &roots, &opts, &mut scratch, &mut Nothing);
        let bare_ms = t.elapsed().as_secs_f64() * 1000.0;
        let labelled: Vec<usize> = probe.cones.iter().map(|c| c.states).collect();
        out["multi"] = json!({
            "walkMs": walk_ms,
            "bareWalkMs": bare_ms,
            "states": stats.states,
            "bareStates": bare.states,
            "maxHops": stats.max_hops,
            "limited": stats.limited,
            "scratchBytes": scratch.bytes(),
            "rootsWithStates": labelled.iter().filter(|n| **n > 0).count(),
            "maxStatesOneRoot": labelled.iter().max(),
        });
    } else {
        let mut per = Vec::new();
        let mut total_ms = 0.0;
        for &r in &roots {
            let mut probe = Probe {
                cones: vec![Cone::default()],
            };
            let t = Instant::now();
            let stats = walk_one(&g, r, &opts, &mut scratch, &mut probe);
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            total_ms += ms;
            let mut s = summarize(&g, r, &probe.cones[0], &object_name);
            s["walkMs"] = json!(ms);
            s["limited"] = json!(stats.limited);
            per.push(s);
        }
        out["walks"] = json!(per);
        out["totalWalkMs"] = json!(total_ms);
        out["scratchBytes"] = json!(scratch.bytes());
    }
    out["rssKiB"] = json!({"beforeWalks": rss_before, "afterWalks": status_kib("VmRSS:"),
                           "peak": status_kib("VmHWM:")});
    Ok(out)
}
