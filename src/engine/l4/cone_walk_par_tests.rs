//! TLS-29 guards (spec `docs/superpowers/specs/2026-11-13-parallel-cone-walk.md` §5).
//!
//! Synthetic graphs from a seeded generator, composed by the serial loop and
//! by the parallel walk (real threads, and the one-thread test scheduler in a
//! chosen pop order), must give the same outcome.

use std::collections::{BTreeMap, HashMap};

use super::{Order, PAR_WALKS, Sched, TEST_SCHED};
use crate::engine::l4::capability_cone::{
    CapabilityFact, ComposeStats, ConeOutcome, CoverageRecord, PdRetain,
    compose_cone_over_graph_retained,
};
use crate::engine::l4::combined_graph::TypedEdge;
use crate::engine::l4::cone_derived::test_facts::{call_edge, fact, temp_fact};
use crate::engine::l4::cone_derived::{ConeDerivedStore, ConeScope, WitnessScope};

/// A synthetic workspace: routines `r/0..n`, call edges, direct facts and
/// direct coverage.
struct Ws {
    nodes: Vec<String>,
    typed: Vec<TypedEdge>,
    direct: HashMap<String, Vec<CapabilityFact>>,
    coverage: HashMap<String, (String, Vec<String>)>,
}

/// splitmix64: a small seeded generator (no dependency).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// Layers of routines; each calls 0-3 routines of lower layers (shared
/// callees), some layers hold recursive groups of 2-30 members (a cycle
/// through the group), some routines call themselves. Facts: table writes,
/// reads and temp facts over a small resource pool, commits, events, IO,
/// and parameter-dependent producers.
fn workspace(seed: u64, layers: usize, width: usize) -> Ws {
    let mut rng = Rng(seed);
    let id = |i: usize| format!("r/{i}");
    let n = layers * width;
    let nodes: Vec<String> = (0..n).map(id).collect();
    let mut typed = Vec::new();
    let mut site = 0usize;
    let mut edge = |typed: &mut Vec<TypedEdge>, from: usize, to: usize| {
        site += 1;
        typed.push(call_edge(&id(from), &id(to), &format!("cs{site}")));
    };
    for layer in 1..layers {
        for k in 0..width {
            let from = layer * width + k;
            for _ in 0..rng.below(4) {
                let to = rng.below((layer * width) as u64) as usize;
                edge(&mut typed, from, to);
            }
        }
        // A recursive group: a cycle over consecutive routines of this layer.
        if rng.below(3) == 0 {
            let len = 2 + rng.below(29.min(width as u64 - 1)) as usize;
            let start = layer * width + rng.below((width - len.min(width)) as u64 + 1) as usize;
            let end = (start + len).min((layer + 1) * width);
            for r in start..end {
                let next = if r + 1 == end { start } else { r + 1 };
                edge(&mut typed, r, next);
            }
        }
        if rng.below(4) == 0 {
            let r = layer * width + rng.below(width as u64) as usize;
            edge(&mut typed, r, r);
        }
    }
    let tables = ["T1", "T2", "T3", "T4", "T5", "T6"];
    let events = ["E1", "E2", "E3"];
    let mut direct: HashMap<String, Vec<CapabilityFact>> = HashMap::new();
    let mut op_id = 0usize;
    for (i, node) in nodes.iter().enumerate() {
        let mut facts = Vec::new();
        for _ in 0..rng.below(4) {
            let t = tables[rng.below(tables.len() as u64) as usize];
            facts.push(match rng.below(8) {
                0 => fact(node, "insert", "table", Some(t)),
                1 => fact(node, "modify", "table", Some(t)),
                2 => fact(node, "read", "table", Some(t)),
                3 => temp_fact(node, "delete", t, "known", Some(true)),
                4 => fact(
                    node,
                    "publish",
                    "event",
                    Some(events[rng.below(3) as usize]),
                ),
                5 => fact(node, "commit", "transaction", None),
                6 => fact(node, "send", "http", None),
                _ => {
                    op_id += 1;
                    let mut f = temp_fact(node, "insert", t, "parameter-dependent", None);
                    f.witness_operation_id = Some(format!("op{op_id}"));
                    f
                }
            });
        }
        if !facts.is_empty() || i % 2 == 0 {
            direct.insert(node.clone(), facts);
        }
    }
    let coverage = nodes
        .iter()
        .map(|n| {
            let c = if rng.below(5) == 0 {
                (
                    "partial".to_string(),
                    vec![format!("reason-{}", rng.below(3))],
                )
            } else {
                ("complete".to_string(), Vec::new())
            };
            (n.clone(), c)
        })
        .collect();
    Ws {
        nodes,
        typed,
        direct,
        coverage,
    }
}

/// A composition's outcome in comparable form: coverage, every derived row
/// resolved to strings, and the stats.
#[derive(Debug, PartialEq)]
struct Seen {
    coverage: BTreeMap<String, CoverageRecord>,
    rows: BTreeMap<String, Row>,
    stats: ComposeStats,
    raw_entries: usize,
}

#[derive(Debug, PartialEq)]
struct Row {
    flags: u8,
    writes: Vec<String>,
    physical_writes: BTreeMap<String, Vec<&'static str>>,
    physical_reads: Vec<String>,
    events: Vec<String>,
}

fn rows_of(store: &ConeDerivedStore) -> BTreeMap<String, Row> {
    store
        .row_ids()
        .into_iter()
        .map(|id| {
            (
                id.to_string(),
                Row {
                    flags: store.flags_of(id),
                    writes: store.writes_tables_of(id),
                    physical_writes: store.physical_table_write_ops_of(id),
                    physical_reads: store.physical_table_reads_of(id),
                    events: store.publishes_events_of(id),
                },
            )
        })
        .collect()
}

fn seen(out: &ConeOutcome) -> Seen {
    Seen {
        coverage: out.coverage.clone().into_iter().collect(),
        rows: rows_of(&out.derived),
        stats: out.stats,
        raw_entries: out.raw.len(),
    }
}

fn derived() -> ConeScope {
    ConeScope {
        derived: true,
        raw: WitnessScope::None,
    }
}

/// Compose `ws` with `scope`; `sched` (if any) is the thread's test schedule.
fn compose(ws: &Ws, scope: ConeScope, workers: usize, sched: Option<Sched>) -> ConeOutcome {
    compose_retained(ws, scope, workers, sched, PdRetain::No)
}

fn compose_retained(
    ws: &Ws,
    scope: ConeScope,
    workers: usize,
    sched: Option<Sched>,
    pd: PdRetain<'_>,
) -> ConeOutcome {
    TEST_SCHED.with(|c| c.set(sched));
    let out = compose_cone_over_graph_retained(
        &ws.typed,
        &ws.nodes,
        &ws.direct,
        &ws.coverage,
        &[],
        scope,
        pd,
        workers,
    );
    TEST_SCHED.with(|c| c.set(None));
    out
}

fn par_walks() -> usize {
    PAR_WALKS.with(std::cell::Cell::get)
}

fn one_thread(order: Order) -> Sched {
    Sched {
        order,
        ..Sched::threads(1)
    }
}

/// Guard 1: the parallel walk gives the serial walk's outcome, at any worker
/// count and in any pop order. Discrimination proof (2026-11-13): an SCC made
/// ready one successor early (`left.fetch_sub(..) <= 2` in the readiness
/// filter) fails here at the start assert ("SCC 26 started before its
/// successor 25 was done"), already in the 2-worker run.
#[test]
fn the_parallel_walk_gives_the_serial_outcome() {
    for seed in [1u64, 7, 42, 1234] {
        let ws = workspace(seed, 12, 40);
        let serial = seen(&compose(&ws, derived(), 1, None));
        assert!(
            serial.rows.len() > 100 && serial.stats.recursive_sccs > 0,
            "seed {seed}: the generated graph is too small to exercise the walk"
        );
        let before = par_walks();
        for workers in [2, 3, 8] {
            let par = seen(&compose(&ws, derived(), workers, None));
            assert_eq!(par, serial, "seed {seed}, {workers} workers");
        }
        for order in [
            Order::Lowest,
            Order::Highest,
            Order::Shuffled(seed),
            Order::Shuffled(seed ^ 0xFFFF),
        ] {
            let par = seen(&compose(&ws, derived(), 1, Some(one_thread(order))));
            assert_eq!(par, serial, "seed {seed}, one thread, {order:?}");
        }
        assert_eq!(
            par_walks() - before,
            7,
            "seed {seed}: every run was parallel"
        );
        // Coverage only (`derived: false`): the members' coverage-only branch.
        let cov_only = || ConeScope {
            derived: false,
            raw: WitnessScope::None,
        };
        let serial = seen(&compose(&ws, cov_only(), 1, None));
        assert!(serial.rows.is_empty() && !serial.coverage.is_empty());
        let par = seen(&compose(&ws, cov_only(), 3, None));
        assert_eq!(par, serial, "seed {seed}: coverage only");
    }
}

/// The guard frames and the producer solve's own entries under the parallel
/// walk, on a real model (review r1, sol and Opus: the synthetic graphs have
/// no routines, so no frame). `G` writes `Rec2` under `Flag`: `N`'s literal
/// `false` contradicts the guard, `M`'s `true` satisfies it, and the
/// recursive SCC {A, B} reaches the guarded entry. `W` is a
/// parameter-dependent producer that the recursive SCC {P, Q} binds.
/// Discrimination proof (2026-11-13): building the task's fact cone with no
/// guard frame (`GuardAnchor { frame: None, .. }`) fails here (G's entry is
/// no longer frame-bound: N's row gains the write, `with_guarded_successor`
/// drops to 0). Passing no frame to the singleton walk instead stays green:
/// no caller here has a guard parameter of its own.
#[test]
fn guards_and_own_entries_match_the_serial_walk() {
    use crate::engine::l4::capability_cone::callsite_local_scan_tests::model;
    use crate::engine::l4::exec_relation::ExecRelation;
    use crate::engine::l5::detector_context::det_cone_inputs;
    let m = model(
        r#"
codeunit 50100 C
{
    var
        Rec2: Record Cust;

    procedure W(Flag: Boolean; var R: Record Cust)
    begin
        if Flag then
            R.Insert();
    end;

    procedure Q(Flag: Boolean)
    var
        R: Record Cust;
    begin
        W(Flag, R);
        P();
    end;

    procedure P()
    begin
        Q(false);
    end;

    procedure A()
    begin
        B();
    end;

    procedure B()
    begin
        G(true);
        A();
    end;

    procedure G(Flag: Boolean)
    begin
        if Flag then
            Rec2.Modify();
    end;

    procedure N()
    begin
        G(false);
    end;

    procedure M()
    begin
        G(true);
    end;
}
"#,
    );
    let rel = ExecRelation::single_app(&m);
    let i = det_cone_inputs(&m, &rel);
    let run = |workers: usize, sched: Option<Sched>| {
        TEST_SCHED.with(|c| c.set(sched));
        let out = compose_cone_over_graph_retained(
            rel.cone(),
            &i.nodes,
            &i.direct_full,
            &i.coverage_in,
            &m.workspace.routines,
            derived(),
            PdRetain::No,
            workers,
        );
        TEST_SCHED.with(|c| c.set(None));
        seen(&out)
    };
    let serial = run(1, None);
    let id = |name: &str| {
        m.workspace
            .routines
            .iter()
            .find(|r| r.name == name)
            .map(|r| r.id.clone())
            .expect("a fixture routine")
    };
    assert!(
        serial.rows[&id("N")].writes.is_empty() && !serial.rows[&id("M")].writes.is_empty(),
        "precondition: the guard decides N's and M's rows"
    );
    assert!(
        serial.stats.with_guarded_successor >= 1 && serial.stats.own_entries_recursive >= 1,
        "precondition: a guarded successor entry and a recursive member's own entry"
    );
    for workers in [2, 8] {
        assert_eq!(run(workers, None), serial, "{workers} workers");
    }
    for order in [Order::Lowest, Order::Highest, Order::Shuffled(3)] {
        assert_eq!(run(1, Some(one_thread(order))), serial, "{order:?}");
    }
}

/// Guard 1 (overlap): with real threads, tasks do run at once, so the
/// equality above is not a serial run in disguise.
#[test]
fn real_threads_overlap() {
    if std::thread::available_parallelism().map_or(1, usize::from) < 2 {
        eprintln!("real_threads_overlap: skipped, one core");
        return;
    }
    let ws = workspace(99, 4, 400);
    let mut most = 0;
    for _ in 0..5 {
        compose(&ws, derived(), 8, None);
        most = most.max(super::LAST_MAX_IN_FLIGHT.with(std::cell::Cell::get));
    }
    assert!(most > 1, "no two tasks ran at once in 5 walks");
}

/// Guard 2: a cone lives until its LAST reader is done. Two readers of one
/// callee, run one at a time with the first finishing before the second
/// starts. Discrimination proof (2026-11-13): taking the cone at the first
/// reader's finish (`fetch_sub(..) == uses` instead of `== 1`) fails here,
/// at the second reader's "has no cone".
#[test]
fn a_cone_lives_until_its_last_reader() {
    // r/0 is called by r/1 and r/2; r/3 calls both (so neither is a root).
    let nodes: Vec<String> = (0..4).map(|i| format!("r/{i}")).collect();
    let typed = vec![
        call_edge("r/1", "r/0", "a"),
        call_edge("r/2", "r/0", "b"),
        call_edge("r/3", "r/1", "c"),
        call_edge("r/3", "r/2", "d"),
    ];
    let direct: HashMap<String, Vec<CapabilityFact>> = nodes
        .iter()
        .map(|n| (n.clone(), vec![fact(n, "insert", "table", Some("T1"))]))
        .collect();
    let ws = Ws {
        coverage: nodes
            .iter()
            .map(|n| (n.clone(), ("complete".to_string(), Vec::new())))
            .collect(),
        nodes,
        typed,
        direct,
    };
    let serial = seen(&compose(&ws, derived(), 1, None));
    for order in [Order::Lowest, Order::Highest] {
        let par = seen(&compose(&ws, derived(), 1, Some(one_thread(order))));
        assert_eq!(par, serial, "{order:?}");
    }
}

/// Guard 3: a raw member or a cone store keeps the serial loop; `Subjects`
/// of an empty set runs the parallel walk and returns the serial walk's
/// (empty) raw table. Discrimination proof (2026-11-13): dropping
/// `scope.raw.names_none()` from the driver's condition fails the raw case
/// (a parallel walk counted); testing `is_none()` instead fails the
/// `Subjects(∅)` case (none counted).
#[test]
fn raw_and_kept_cones_stay_serial() {
    let ws = workspace(5, 6, 30);
    let member = ws.nodes[ws.nodes.len() - 1].clone();
    let raw_scope = || ConeScope {
        derived: true,
        raw: WitnessScope::Subjects([member.clone()].into_iter().collect()),
    };
    let before = par_walks();
    let serial = seen(&compose(&ws, raw_scope(), 1, None));
    let eight = seen(&compose(&ws, raw_scope(), 8, None));
    assert_eq!(eight, serial);
    assert_eq!(par_walks(), before, "a raw member ran the parallel walk");

    let kept = |workers| {
        compose_retained(
            &ws,
            derived(),
            workers,
            None,
            PdRetain::Yes {
                seed: None,
                cones: true,
            },
        )
    };
    let kept_serial = seen(&kept(1));
    let kept_eight = seen(&kept(8));
    assert_eq!(kept_eight, kept_serial);
    assert_eq!(par_walks(), before, "a cone store ran the parallel walk");

    let empty = || ConeScope {
        derived: true,
        raw: WitnessScope::Subjects(Default::default()),
    };
    let empty_serial = seen(&compose(&ws, empty(), 1, None));
    let empty_par = seen(&compose(&ws, empty(), 8, None));
    assert_eq!(par_walks(), before + 1, "Subjects(∅) ran the serial loop");
    assert_eq!(empty_par, empty_serial);
    assert_eq!(empty_par.raw_entries, 0);
}

/// Guard 4: a panicking task does not hang the walk. Every other SCC
/// depends on the leaf `r/0`, so while it runs the other workers park; its
/// task waits for them, then panics. Discrimination proof (2026-11-13):
/// without `AbortOnPanic`'s notify the parked workers never wake and the
/// walk misses the deadline.
#[test]
fn a_panicking_task_does_not_hang_the_walk() {
    let nodes: Vec<String> = (0..6).map(|i| format!("r/{i}")).collect();
    let typed: Vec<TypedEdge> = (1..6)
        .map(|i| call_edge(&format!("r/{i}"), "r/0", &format!("s{i}")))
        .collect();
    let ws = Ws {
        direct: HashMap::new(),
        coverage: nodes
            .iter()
            .map(|n| (n.clone(), ("complete".to_string(), Vec::new())))
            .collect(),
        nodes,
        typed,
    };
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let sched = Sched {
            panic_first: true,
            ..Sched::threads(3)
        };
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            compose(&ws, derived(), 3, Some(sched))
        }));
        let _ = tx.send(r.is_err());
    });
    let panicked = rx
        .recv_timeout(std::time::Duration::from_secs(20))
        .expect("the walk hung after a task panicked");
    assert!(panicked, "the injected panic was not raised");
}
