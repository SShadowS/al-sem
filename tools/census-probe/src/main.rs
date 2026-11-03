//! Byte census of the program graph + LSP state, per workspace root.
//!
//! Accounting: a counting global allocator over `System`. Live bytes are the
//! REQUESTED sizes (allocator overhead excluded); counts are live allocations.
//! Q1/Q4/Q5 numbers are DROP DELTAS (exact: bytes freed when a field drops,
//! in the printed order; an Arc held elsewhere frees 0 until its last holder).
//! Q2 numbers are a WALK: String/Box<str> by len (not capacity), Vec backing by
//! len * size_of, Arc<str> texts counted once per pointer.
//!
//! Usage: census-probe <embedded|symbols> <root> [<root> ...]
//! All roots share ONE DepCache, built sequentially, kept alive (like the server).

use std::alloc::{GlobalAlloc, Layout};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::mem::size_of;
use std::path::PathBuf;
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Mutex};

use al_sem::lsp::encoding::PositionEncoding;
use al_sem::lsp::snapshot::{DeclEntry, EdgeRef, LspEdge, LspSnapshot, LspTarget, ParsedFileEntry};
use al_sem::program::build::DepLayer;
use al_sem::program::dep_cache::{DepCache, DepNodes};
use al_sem::program::graph::ProgramGraph;
use al_sem::program::node::{ObjKey, ObjectNodeId, RoutineNodeId};
use al_sem::program::node_extract::{AbiParams, ObjectNode, ObjectRef, RoutineNode};
use al_sem::program::resolve::decl_surface::RoutineMeta;
use al_sem::program::resolve::edge::{AbiRoutineKey, Route};
use al_sem::program::resolve::full::ClassifiedEdge;
use al_sem::snapshot::DependencySource;
use al_sem::snapshot::parse::ParsedUnit;

// ───────────────────────── counting allocator ─────────────────────────

struct Counting;
static BYTES: AtomicIsize = AtomicIsize::new(0);
static ALLOCS: AtomicIsize = AtomicIsize::new(0);
static PEAK: AtomicIsize = AtomicIsize::new(0);
/// Whether ≥ [`BIG`] requests are logged (`refresh-seq` turns it off: a
/// long run would grow the log, which the live heap counts).
static LOG_BIG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// The allocator the counting wrapper forwards to: `System`, or mimalloc
/// with `--features mimalloc` (the resident child's).
#[cfg(not(feature = "mimalloc"))]
static INNER: std::alloc::System = std::alloc::System;
#[cfg(feature = "mimalloc")]
static INNER: al_sem::release_alloc::ReleasingMiMalloc = al_sem::release_alloc::ReleasingMiMalloc;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let n = BYTES.fetch_add(l.size() as isize, Relaxed) + l.size() as isize;
        PEAK.fetch_max(n, Relaxed);
        ALLOCS.fetch_add(1, Relaxed);
        if l.size() >= BIG && LOG_BIG.load(Relaxed) {
            log_big("alloc", l.size(), 0, n);
        }
        unsafe { INNER.alloc(l) }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        let n = BYTES.fetch_add(l.size() as isize, Relaxed) + l.size() as isize;
        PEAK.fetch_max(n, Relaxed);
        ALLOCS.fetch_add(1, Relaxed);
        if l.size() >= BIG && LOG_BIG.load(Relaxed) {
            log_big("alloc_zeroed", l.size(), 0, n);
        }
        unsafe { INNER.alloc_zeroed(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        BYTES.fetch_sub(l.size() as isize, Relaxed);
        ALLOCS.fetch_sub(1, Relaxed);
        unsafe { INNER.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        let n = BYTES.fetch_add(new as isize - l.size() as isize, Relaxed) + new as isize
            - l.size() as isize;
        PEAK.fetch_max(n, Relaxed);
        if new >= BIG && LOG_BIG.load(Relaxed) {
            log_big("realloc", new, l.size(), n);
        }
        unsafe { INNER.realloc(p, l, new) }
    }
}

// ───────────────────────── large-allocation log (issue #84) ─────────────────────────

/// Every alloc/alloc_zeroed/realloc whose NEW requested size is at least this
/// is logged with a backtrace.
const BIG: usize = 8 << 20;

struct Big {
    kind: &'static str,
    new: usize,
    /// The block's old size (realloc only; 0 otherwise).
    old: usize,
    /// Live heap right after this request was counted (net: old block already
    /// subtracted for a realloc).
    live: isize,
    /// 1-based root being built (0 = outside a root build).
    root: usize,
    /// Number of census marks seen so far in this root's build: the record
    /// belongs to the phase that ends at mark `phase`.
    phase: usize,
    /// Captured UNRESOLVED; resolved only when printed.
    bt: backtrace::Backtrace,
}

/// Guard-protected buffer: only pushed while the thread's `IN` guard is set,
/// so the push's own allocation (and the backtrace's) re-enter the allocator
/// with the guard set and are never logged.
static BIGS: Mutex<Vec<Big>> = Mutex::new(Vec::new());
static ROOT: AtomicUsize = AtomicUsize::new(0);
static PHASE: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    // const + no Drop: allocation-free access, never torn down.
    static IN: Cell<bool> = const { Cell::new(false) };
}

fn log_big(kind: &'static str, new: usize, old: usize, live: isize) {
    let entered = IN
        .try_with(|g| {
            if g.get() {
                false
            } else {
                g.set(true);
                true
            }
        })
        .unwrap_or(false);
    if !entered {
        return;
    }
    let rec = Big {
        kind,
        new,
        old,
        live,
        root: ROOT.load(Relaxed),
        phase: PHASE.load(Relaxed),
        bt: backtrace::Backtrace::new_unresolved(),
    };
    BIGS.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(rec);
    let _ = IN.try_with(|g| g.set(false));
}

/// Prints and drains the ≥8 MiB records of root `root`. `marks` are this
/// root's phase names in order; `b0` its live heap before the build.
fn report_bigs(root: usize, marks: &[&'static str], b0: isize) {
    // Take the records out and drop the lock first: nothing below may
    // allocate while BIGS is held (the allocator hook locks it too).
    let all: Vec<Big> = std::mem::take(
        &mut *BIGS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    let (mine, rest): (Vec<Big>, Vec<Big>) = all.into_iter().partition(|r| r.root == root);
    // Put the other roots' records back, AFTER any logged since the take, so
    // none is lost. `extend` may allocate under the lock, so set the guard
    // first: that allocation re-enters `log_big` unlogged and cannot try to
    // take the lock this thread already holds.
    let _ = IN.try_with(|g| g.set(true));
    BIGS.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .extend(rest);
    let _ = IN.try_with(|g| g.set(false));
    let (rn, on) = (size_of::<RoutineNode>(), size_of::<ObjectNode>());
    println!(
        "Large-allocation log: {} request(s) with NEW size >= {} B during this build (live = net live heap after the request, relative to before this root's build; a realloc's old and new blocks are both live during the copy, so its transient is live + old)",
        mine.len(),
        BIG
    );
    let mut largest: Option<(usize, &'static str, usize)> = None;
    for (i, mut r) in mine.into_iter().enumerate() {
        r.bt.resolve();
        let mut frames: Vec<String> = Vec::new();
        let mut in_build = false;
        for f in r.bt.frames() {
            for s in f.symbols() {
                let name = s.name().map(|n| format!("{n:#}")).unwrap_or_default();
                if name.contains("build_dep_nodes")
                    || name.contains("dedup_routines_preserving_genuine_overloads")
                {
                    in_build = true;
                }
                if name.contains("al_sem") {
                    let at = match (s.filename(), s.lineno()) {
                        (Some(p), Some(l)) => format!(" at {}:{l}", p.display()),
                        _ => String::new(),
                    };
                    frames.push(format!("{name}{at}"));
                }
            }
        }
        let mut flags: Vec<String> = Vec::new();
        if in_build && r.new % rn == 0 {
            flags.push(format!("Vec<RoutineNode> cap {}", r.new / rn));
        }
        if in_build && r.new % on == 0 {
            flags.push(format!("Vec<ObjectNode> cap {}", r.new / on));
        }
        if in_build && !flags.is_empty() && largest.is_none_or(|l| r.new > l.0) {
            largest = Some((r.new, r.kind, i + 1));
        }
        let phase = marks.get(r.phase).copied().unwrap_or("(after last mark)");
        println!(
            "  #{} {:<12} new {:>12} B ({:.2} MiB)  old {:>12} B  live {:.1} MiB  transient {:.1} MiB  phase {}  through build_dep_nodes/dedup: {}  {}",
            i + 1,
            r.kind,
            r.new,
            mibu(r.new as u64),
            r.old,
            mib(r.live - b0),
            mib(r.live - b0 + r.old as isize),
            phase,
            in_build,
            if flags.is_empty() {
                String::new()
            } else {
                format!("[{}]", flags.join(" | "))
            }
        );
        for f in &frames {
            println!("      {f}");
        }
    }
    match largest {
        Some((n, k, i)) => println!(
            "  LARGEST Vec<RoutineNode>/Vec<ObjectNode> buffer in build_dep_nodes: #{i} {k} {n} B ({:.2} MiB)",
            mibu(n as u64)
        ),
        None => println!(
            "  LARGEST Vec<RoutineNode>/Vec<ObjectNode> buffer in build_dep_nodes: none >= {BIG} B"
        ),
    }
}

#[global_allocator]
static A: Counting = Counting;

fn live() -> (isize, isize) {
    (BYTES.load(Relaxed), ALLOCS.load(Relaxed))
}
fn reset_peak() {
    PEAK.store(BYTES.load(Relaxed), Relaxed);
}
fn mib(b: isize) -> f64 {
    b as f64 / 1_048_576.0
}
fn mibu(b: u64) -> f64 {
    b as f64 / 1_048_576.0
}

/// Wait until the live heap stops moving (e.g. an updater's background work),
/// so a reading is not taken mid-change.
fn settle() {
    let mut last = BYTES.load(Relaxed);
    let mut stable = 0;
    while stable < 3 {
        std::thread::sleep(std::time::Duration::from_millis(300));
        let cur = BYTES.load(Relaxed);
        if cur == last {
            stable += 1;
        } else {
            stable = 0;
            last = cur;
        }
    }
}

// ───────────────────────── phase marks (Q3) ─────────────────────────

struct Mark {
    name: &'static str,
    live: isize,
    peak: isize,
    allocs: isize,
}
static MARKS: Mutex<Vec<Mark>> = Mutex::new(Vec::new());

fn on_mark(name: &'static str) {
    let (b, a) = live();
    let p = PEAK.load(Relaxed);
    MARKS.lock().unwrap().push(Mark {
        name,
        live: b,
        peak: p,
        allocs: a,
    });
    PHASE.fetch_add(1, Relaxed);
    reset_peak();
}

// ───────────────────────── drop steps (Q1/Q4/Q5) ─────────────────────────

fn step<T>(label: &str, v: T) -> (isize, isize) {
    let (b0, a0) = live();
    drop(v);
    let (b1, a1) = live();
    println!(
        "  {label:<58} {:>9.2} MiB {:>10} allocs",
        mib(b0 - b1),
        a0 - a1
    );
    (b0 - b1, a0 - a1)
}

struct Root {
    path: PathBuf,
    snap: LspSnapshot,
    ws: ParsedUnit,
    /// compute_all's HashMap<String, Vec<lsp_types::Diagnostic>>, boxed once (opaque).
    diags: Box<dyn std::any::Any>,
}

fn drop_root_by_field(r: Root) {
    println!("drop-delta breakdown for {}:", r.path.display());
    let base = live();
    let Root {
        snap, ws, diags, ..
    } = r;
    step("diagnostics (compute_all output)", diags);
    step(
        "updater workspace ParsedUnit (shells; Arcs shared w/ parsed)",
        ws,
    );
    let LspSnapshot {
        graph,
        dep_layer,
        snap,
        parsed,
        edges_by_file,
        ws_event_edges,
        dep_events,
        ws_incoming,
        ws_publisher_fanout,
        decls_by_file,
        decl_by_id,
        dep_lines,
        dep_meta,
        workspace_root,
        ..
    } = snap;
    step("snapshot.dep_meta (Arc clone)", dep_meta);
    step("snapshot.dep_lines (Arc clone)", dep_lines);
    step("snapshot.dep_events (Arc clone)", dep_events);
    step("decl_by_id", decl_by_id);
    step("decls_by_file", decls_by_file);
    // S10.4: these three now hold only the workspace part of the event links.
    step("incoming", ws_incoming);
    step("publisher_fanout", ws_publisher_fanout);
    step("edges_by_file", edges_by_file);
    step("event_edges", ws_event_edges);
    step("parsed (ws AlFile IR + text + LineTable)", parsed);
    match Arc::try_unwrap(graph) {
        Ok(g) => {
            let ProgramGraph {
                apps,
                topology,
                objects,
                routines,
                obj_index,
                friends,
                abi_ingest_errors,
                workspace_rows,
                ..
            } = g;
            step("graph.objects (own part + own_pos)", objects);
            step("graph.workspace_rows", workspace_rows);
            step("graph.routines (own part + own_pos)", routines);
            step("graph.obj_index", obj_index);
            step("graph.apps", apps);
            step("graph.topology", topology);
            step("graph.friends", friends);
            step("graph.abi_ingest_errors", abi_ingest_errors);
        }
        Err(g) => {
            step("graph (Arc held elsewhere)", g);
        }
    }
    match Arc::try_unwrap(dep_layer) {
        Ok(d) => {
            let DepLayer {
                apps,
                topology,
                friends,
                dep_objects,
                dep_routines,
                abi_ingest_errors,
                dep_nodes,
            } = d;
            step(
                "dep_layer.apps/topology/friends/errors",
                (apps, topology, friends, abi_ingest_errors),
            );
            step(
                "dep_layer.dep_objects/routines (Arc clones)",
                (dep_objects, dep_routines),
            );
            match Arc::try_unwrap(dep_nodes) {
                Ok(n) => {
                    let DepNodes {
                        objects,
                        routines,
                        abi_ingest_errors,
                        dep_meta,
                        recovered,
                        bodies,
                        lsp,
                        lsp_events,
                    } = n;
                    step(
                        "SHARED dep tier: dependency event links",
                        lsp_events.into_inner(),
                    );
                    step("SHARED dep tier: objects", objects);
                    step("SHARED dep tier: routines", routines);
                    step("SHARED dep tier: abi_ingest_errors", abi_ingest_errors);
                    // dep_meta moved from DepLspTier to DepNodes; same label.
                    step("SHARED dep tier: dep_meta", dep_meta);
                    step("SHARED dep tier: recovered", recovered);
                    // New field: dependency ParsedUnits, Some only under Keep (FULL).
                    step("SHARED dep tier: bodies (Keep only)", bodies);
                    match lsp.into_inner().map(Arc::try_unwrap) {
                        Some(Ok(t)) => {
                            step("SHARED dep tier: dep_lines", t.dep_lines);
                        }
                        Some(Err(t)) => {
                            step("SHARED dep tier: lsp (held elsewhere)", t);
                        }
                        None => {}
                    }
                }
                Err(n) => {
                    step("dep_nodes (held by other roots)", n);
                }
            }
        }
        Err(d) => {
            step("dep_layer (Arc held elsewhere)", d);
        }
    }
    match Arc::try_unwrap(snap) {
        Ok(mut s) => {
            let mut abi = Vec::new();
            let mut src = Vec::new();
            for u in s.apps.iter_mut() {
                abi.push(u.abi.take());
                src.push(u.source.take());
            }
            step(
                "snap: ABI packages (ParsedAppPackage; shared via DepCache)",
                abi,
            );
            step(
                "snap: source roots (SourceFile texts; shared via DepCache)",
                src,
            );
            step("snap: rest", s);
        }
        Err(s) => {
            step("snap (held elsewhere)", s);
        }
    }
    step("workspace_root", workspace_root);
    let now = live();
    println!(
        "  TOTAL freed by this root: {:.2} MiB in {} allocs",
        mib(base.0 - now.0),
        base.1 - now.1
    );
}

// ───────────────────────── walker (Q2) ─────────────────────────

type Key = (&'static str, &'static str);

#[derive(Default)]
struct StrStat<'a> {
    count: u64,
    bytes: u64,
    distinct: HashMap<&'a str, u32>,
    /// Text allocations actually held (S10.2: a `SharedStr` shares one), by
    /// data pointer -> length.
    allocs: HashMap<usize, u64>,
}

#[derive(Default)]
struct VecStat {
    count: u64,
    bytes: u64,
}

#[derive(Default)]
struct W<'a> {
    strs: HashMap<Key, StrStat<'a>>,
    global: HashMap<&'a str, u32>,
    global_allocs: HashMap<usize, u64>,
    vecs: HashMap<Key, VecStat>,
    /// inline element bytes of containers: (structure) -> (elements, bytes)
    elems: HashMap<&'static str, (u64, u64)>,
    rid_copies: HashMap<Key, u64>,
    rid_global: HashMap<&'a RoutineNodeId, u32>,
    oid_copies: HashMap<Key, u64>,
    oid_global: HashMap<&'a ObjectNodeId, u32>,
    seen: HashSet<usize>,
    /// Arc<str> texts, once per pointer: label -> (count, bytes)
    texts: HashMap<&'static str, (u64, u64)>,
    by_kind: HashMap<(&'static str, String), (u64, u64)>, // (structure, kind) -> (n, string+vec heap)
}

impl<'a> W<'a> {
    fn first<T: ?Sized>(&mut self, p: *const T) -> bool {
        self.seen.insert(p as *const () as usize)
    }
    fn s(&mut self, k: Key, v: &'a str) -> u64 {
        if v.is_empty() {
            return 0;
        }
        let st = self.strs.entry(k).or_default();
        st.count += 1;
        st.bytes += v.len() as u64;
        *st.distinct.entry(v).or_default() += 1;
        st.allocs.insert(v.as_ptr() as usize, v.len() as u64);
        *self.global.entry(v).or_default() += 1;
        self.global_allocs
            .insert(v.as_ptr() as usize, v.len() as u64);
        v.len() as u64
    }
    fn os<S: AsRef<str>>(&mut self, k: Key, v: &'a Option<S>) -> u64 {
        v.as_ref().map_or(0, |x| self.s(k, x.as_ref()))
    }
    fn v<T>(&mut self, k: Key, v: &[T]) -> u64 {
        if v.is_empty() {
            return 0;
        }
        let st = self.vecs.entry(k).or_default();
        st.count += 1;
        let b = (v.len() * size_of::<T>()) as u64;
        st.bytes += b;
        b
    }
    fn elems<T>(&mut self, s: &'static str, n: usize) {
        let e = self.elems.entry(s).or_default();
        e.0 += n as u64;
        e.1 += (n * size_of::<T>()) as u64;
    }
    fn text(&mut self, label: &'static str, t: &Arc<str>) {
        if self.first(Arc::as_ptr(t)) {
            let e = self.texts.entry(label).or_default();
            e.0 += 1;
            e.1 += t.len() as u64;
        }
    }
    fn oid(&mut self, k: Key, o: &'a ObjectNodeId) -> u64 {
        *self.oid_copies.entry(k).or_default() += 1;
        *self.oid_global.entry(o).or_default() += 1;
        match &o.key {
            ObjKey::Name(n) => self.s((k.0, "objkey(name)"), n),
            ObjKey::Id(_) => 0,
        }
    }
    fn rid(&mut self, k: Key, r: &'a RoutineNodeId) -> u64 {
        *self.rid_copies.entry(k).or_default() += 1;
        *self.rid_global.entry(r).or_default() += 1;
        let mut b = self.s((k.0, "rid.name_lc"), &r.name_lc);
        b += self.os((k.0, "rid.enclosing_member_lc"), &r.enclosing_member_lc);
        // the ObjectNodeId inside a RoutineNodeId: counted as its own copy
        *self.oid_copies.entry((k.0, "rid.object")).or_default() += 1;
        *self.oid_global.entry(&r.object).or_default() += 1;
        if let ObjKey::Name(n) = &r.object.key {
            b += self.s((k.0, "rid.object.objkey(name)"), n);
        }
        b
    }
    fn objref(&mut self, s: &'static str, o: &'a ObjectRef) -> u64 {
        match o {
            ObjectRef::Name { raw, normalized_lc } => {
                self.s((s, "objref.raw"), raw) + self.s((s, "objref.normalized_lc"), normalized_lc)
            }
            ObjectRef::Id(_) => 0,
        }
    }
    fn object(&mut self, s: &'static str, o: &'a ObjectNode) {
        let mut b = self.oid((s, "id"), &o.id);
        b += self.s((s, "name"), &o.name);
        b += self.os((s, "extends_target"), &o.extends_target);
        b += self.v((s, "implements[]"), &o.implements);
        for i in &o.implements {
            b += self.s((s, "implements"), i);
        }
        if let Some(r) = &o.source_table {
            b += self.objref(s, r);
        }
        if let Some(r) = &o.table_no {
            b += self.objref(s, r);
        }
        b += self.v((s, "page_controls[]"), &o.page_controls);
        for p in &o.page_controls {
            b += self.s((s, "page_control.name_lc"), &p.name_lc);
            b += self.objref(s, &p.target);
        }
        b += self.v((s, "fields[]"), &o.fields);
        for f in &o.fields {
            b += self.s((s, "field.name_lc"), &f.name_lc);
            b += self.s((s, "field.type_text"), &f.type_text);
        }
        b += self.v((s, "dataitems[]"), &o.dataitems);
        for d in &o.dataitems {
            b += self.s((s, "dataitem.name_lc"), &d.name_lc);
            b += self.s((s, "dataitem.name"), &d.name);
            b += self.objref(s, &d.source_table);
        }
        let e = self
            .by_kind
            .entry((s, format!("{:?}", o.id.kind)))
            .or_default();
        e.0 += 1;
        e.1 += b + size_of::<ObjectNode>() as u64;
    }
    fn routine(&mut self, s: &'static str, r: &'a RoutineNode) {
        let mut b = self.rid((s, "id"), &r.id);
        b += self.s((s, "name"), &r.name);
        b += self.v((s, "event_subscribers[]"), &r.event_subscribers);
        for e in &r.event_subscribers {
            b += self.s((s, "sub.publisher_object_type"), &e.publisher_object_type);
            b += self.s((s, "sub.publisher_name"), &e.publisher_name);
            b += self.s((s, "sub.event_name"), &e.event_name);
            b += self.os((s, "sub.element"), &e.element);
        }
        b += self.s((s, "param_sig_key"), &r.param_sig_key);
        b += self.os((s, "return_type"), &r.return_type);
        if let Some((t, _)) = &r.return_type_id {
            b += self.s((s, "return_type_id.0"), t);
        }
        if let AbiParams::Complete(ps) = &r.abi_params {
            b += self.v((s, "abi_params[]"), ps);
            for p in ps {
                b += self.s((s, "abi_param.type_text"), &p.type_text);
                b += self.os((s, "abi_param.subtype_raw_name"), &p.subtype_raw_name);
            }
        }
        let e = self
            .by_kind
            .entry((s, format!("{:?}", r.id.object.kind)))
            .or_default();
        e.0 += 1;
        e.1 += b + size_of::<RoutineNode>() as u64;
    }
    /// S10.3: a `DepMeta` entry's key is the tier row's own id (counted under
    /// `dep.routine`), so only the value is walked here.
    fn meta(&mut self, s: &'static str, m: &'a RoutineMeta) {
        self.s((s, "name"), &m.name);
        self.os((s, "enclosing_member"), &m.enclosing_member);
        self.v((s, "params[]"), &m.params);
        for p in &m.params {
            self.os((s, "param.ty"), &p.ty);
        }
        self.s((s, "virtual_path"), &m.virtual_path);
    }
    fn abikey(&mut self, s: &'static str, k: &'a AbiRoutineKey) {
        self.s((s, "abikey.object_type"), &k.object_type);
        self.s((s, "abikey.object_name_lc"), &k.object_name_lc);
        self.s((s, "abikey.routine_name_lc"), &k.routine_name_lc);
    }
    /// S10.5: the LSP's stored edge (`LspEdge`).
    fn cedge(&mut self, s: &'static str, e: &'a LspEdge) {
        self.rid((s, "edge.from"), &e.from);
        self.s((s, "edge.span.unit"), &e.span.unit);
        self.v((s, "targets[]"), &e.targets);
        for t in e.targets.iter() {
            match t {
                LspTarget::Routine(id) => {
                    self.rid((s, "target"), id);
                }
                LspTarget::Abi(key) => {
                    self.v((s, "target.abi(Box)"), std::slice::from_ref(&**key));
                    self.abikey(s, key);
                }
            }
        }
    }
    fn decl(&mut self, s: &'static str, d: &'a DeclEntry) {
        self.rid((s, "id"), &d.id);
        self.s((s, "name"), &d.name);
        self.s((s, "virtual_path"), &d.virtual_path);
    }

    fn snapshot(&mut self, l: &'a LspSnapshot, ws: &'a ParsedUnit) {
        let g = &*l.graph;
        // graph nodes: the shared (dep) part once per Arc, the own part per root
        if self.first(Arc::as_ptr(g.objects.shared())) {
            self.elems::<ObjectNode>("dep.object", g.objects.shared().len());
            for o in g.objects.shared().iter() {
                self.object("dep.object", o);
            }
        }
        self.elems::<ObjectNode>("ws.object", g.objects.own().len());
        for o in g.objects.own() {
            self.object("ws.object", o);
        }
        if self.first(Arc::as_ptr(g.routines.shared())) {
            self.elems::<RoutineNode>("dep.routine", g.routines.shared().len());
            for r in g.routines.shared().iter() {
                self.routine("dep.routine", r);
            }
        }
        self.elems::<RoutineNode>("ws.routine", g.routines.own().len());
        for r in g.routines.own() {
            self.routine("ws.routine", r);
        }
        if self.first(Arc::as_ptr(&l.dep_meta)) {
            // S10.3: a column of metas plus a `u32` row per meta (orphans,
            // expected none, are reported in the shape line).
            self.elems::<RoutineMeta>("dep_meta", l.dep_meta.len());
            self.elems::<u32>("dep_meta.key_row", l.dep_meta.len());
            for m in l.dep_meta.values() {
                self.meta("dep_meta", m);
            }
        }
        // S10.1: a text-free line index per dependency file (its own heap is
        // measured by the drop steps, not walked here).
        if self.first(Arc::as_ptr(&l.dep_lines)) {
            self.elems::<(
                (
                    al_sem::program::node::AppRef,
                    al_sem::program::node::SharedStr,
                ),
                al_sem::lsp::encoding::LineIndex,
            )>("dep_lines", l.dep_lines.len());
            for ((_, vp), _) in l.dep_lines.iter() {
                self.s(("dep_lines", "key.virtual_path"), vp);
            }
        }
        for (k, v) in &l.edges_by_file {
            self.s(("edges_by_file", "key"), k);
            if self.first(Arc::as_ptr(v)) {
                self.elems::<LspEdge>("edges_by_file", v.len());
                for ce in v.iter() {
                    self.cedge("edges_by_file", ce);
                }
            }
        }
        // S10.4: `event_edges`/`incoming`/`publisher_fanout` are the
        // workspace part; the shared dependency part is walked once, as
        // `dep_events.*`.
        if self.first(Arc::as_ptr(&l.ws_event_edges)) {
            self.elems::<LspEdge>("event_edges", l.ws_event_edges.len());
            for ce in l.ws_event_edges.iter() {
                self.cedge("event_edges", ce);
            }
        }
        self.elems::<(RoutineNodeId, Vec<EdgeRef>)>("incoming", l.ws_incoming.len());
        for (k, v) in &l.ws_incoming {
            self.rid(("incoming", "key"), k);
            self.v(("incoming", "edgerefs[]"), v);
            for e in v {
                self.text("incoming.edgeref.file(Arc<str>)", &e.file);
            }
        }
        if self.first(Arc::as_ptr(&l.ws_publisher_fanout)) {
            self.elems::<(RoutineNodeId, usize)>("publisher_fanout", l.ws_publisher_fanout.len());
            for k in l.ws_publisher_fanout.keys() {
                self.rid(("publisher_fanout", "key"), k);
            }
        }
        if self.first(Arc::as_ptr(&l.dep_events)) {
            let d = &l.dep_events;
            self.elems::<LspEdge>("dep_events.edges", d.edges.len());
            for ce in &d.edges {
                self.cedge("dep_events.edges", ce);
            }
            self.elems::<(RoutineNodeId, Vec<EdgeRef>)>("dep_events.incoming", d.incoming.len());
            for (k, v) in &d.incoming {
                self.rid(("dep_events.incoming", "key"), k);
                self.v(("dep_events.incoming", "edgerefs[]"), v);
            }
            self.elems::<(RoutineNodeId, usize)>(
                "dep_events.publisher_fanout",
                d.publisher_fanout.len(),
            );
            for k in d.publisher_fanout.keys() {
                self.rid(("dep_events.publisher_fanout", "key"), k);
            }
        }
        for (k, v) in &l.decls_by_file {
            self.s(("decls_by_file", "key"), k);
            if self.first(Arc::as_ptr(v)) {
                self.elems::<DeclEntry>("decls_by_file", v.len());
                for d in v.iter() {
                    self.decl("decls_by_file", d);
                }
            }
        }
        self.elems::<(RoutineNodeId, DeclEntry)>("decl_by_id", l.decl_by_id.len());
        for (k, d) in &l.decl_by_id {
            self.rid(("decl_by_id", "key"), k);
            self.decl("decl_by_id", d);
        }
        for (k, p) in &l.parsed {
            self.s(("parsed", "key"), k);
            if self.first(Arc::as_ptr(p)) {
                self.s(("parsed", "virtual_path"), &p.virtual_path);
                self.text("workspace text", &p.text);
            }
        }
        for f in &ws.files {
            self.s(("ws ParsedUnit", "virtual_path"), &f.virtual_path);
            self.text("workspace text", &f.text);
        }
        if self.first(Arc::as_ptr(&l.snap)) {
            for u in &l.snap.apps {
                if let Some(src) = &u.source
                    && self.first(Arc::as_ptr(&src.files))
                {
                    for f in src.files.iter() {
                        self.s(("snap.source", "virtual_path"), &f.virtual_path);
                        self.text(
                            if u.id == l.snap.workspace_app {
                                "workspace text"
                            } else {
                                "dep source text (snap)"
                            },
                            &f.text,
                        );
                    }
                }
            }
        }
    }

    fn report(&self, title: &str) {
        println!("\n==== Q2 walk: {title} ====");
        println!(
            "-- strings by (structure, field), len bytes; 'saved' = bytes/allocs removed by interning WITHIN the field"
        );
        println!(
            "-- 'held' = text allocations actually held, once per data pointer (a shared string, S10.2, counts once)"
        );
        println!(
            "  {:<22} {:<30} {:>10} {:>9} {:>10} {:>9} {:>9} {:>9} {:>10} {:>9}",
            "structure",
            "field",
            "count",
            "MiB",
            "distinct",
            "dist MiB",
            "saved",
            "save%",
            "held",
            "held MiB"
        );
        let mut rows: Vec<_> = self.strs.iter().collect();
        rows.sort_by(|a, b| b.1.bytes.cmp(&a.1.bytes));
        let (mut tc, mut tb) = (0u64, 0u64);
        for ((s, f), st) in &rows {
            let db: u64 = st.distinct.keys().map(|k| k.len() as u64).sum();
            tc += st.count;
            tb += st.bytes;
            println!(
                "  {:<22} {:<30} {:>10} {:>9.2} {:>10} {:>9.2} {:>9.2} {:>8.1}% {:>10} {:>9.2}",
                s,
                f,
                st.count,
                mibu(st.bytes),
                st.distinct.len(),
                mibu(db),
                mibu(st.bytes - db),
                100.0 * (st.bytes - db) as f64 / st.bytes.max(1) as f64,
                st.allocs.len(),
                mibu(st.allocs.values().sum())
            );
        }
        let gdb: u64 = self.global.keys().map(|k| k.len() as u64).sum();
        println!(
            "  ALL STRINGS: {} strings, {:.2} MiB; GLOBAL distinct {} ({:.2} MiB) => interning saves {:.2} MiB string bytes and {} allocs (+{:.2} MiB at 24 B/alloc LFH overhead)",
            tc,
            mibu(tb),
            self.global.len(),
            mibu(gdb),
            mibu(tb - gdb),
            tc - self.global.len() as u64,
            mibu((tc - self.global.len() as u64) * 24)
        );
        println!(
            "  HELD: {} text allocations, {:.2} MiB (all strings above, each allocation counted once)",
            self.global_allocs.len(),
            mibu(self.global_allocs.values().sum())
        );
        // String headers (24 B each) are inline in their parent: counted under elems/vecs.

        println!("-- Vec backing buffers (len * size_of)");
        let mut vrows: Vec<_> = self.vecs.iter().collect();
        vrows.sort_by(|a, b| b.1.bytes.cmp(&a.1.bytes));
        for ((s, f), v) in vrows {
            println!(
                "  {:<22} {:<30} {:>10} vecs {:>9.2} MiB",
                s,
                f,
                v.count,
                mibu(v.bytes)
            );
        }
        println!("-- container elements (inline struct bytes: count * size_of)");
        let mut erows: Vec<_> = self.elems.iter().collect();
        erows.sort_by(|a, b| b.1.1.cmp(&a.1.1));
        for (s, (n, b)) in erows {
            println!("  {:<30} {:>10} elems {:>9.2} MiB", s, n, mibu(*b));
        }
        println!("-- Arc<str> texts (once per pointer)");
        for (l, (n, b)) in &self.texts {
            println!("  {:<40} {:>8} texts {:>9.2} MiB", l, n, mibu(*b));
        }
        println!("-- per node kind (n, inline + owned heap MiB)");
        let mut krows: Vec<_> = self.by_kind.iter().collect();
        krows.sort_by(|a, b| b.1.1.cmp(&a.1.1));
        for ((s, k), (n, b)) in krows {
            println!("  {:<14} {:<22} {:>9} {:>9.2} MiB", s, k, n, mibu(*b));
        }

        println!(
            "-- RoutineNodeId copies by (structure, field); inline {} B each",
            size_of::<RoutineNodeId>()
        );
        let mut rrows: Vec<_> = self.rid_copies.iter().collect();
        rrows.sort_by(|a, b| b.1.cmp(a.1));
        let mut total = 0u64;
        for ((s, f), n) in rrows {
            total += n;
            println!(
                "  {:<22} {:<26} {:>10} copies {:>9.2} MiB inline",
                s,
                f,
                n,
                mibu(n * size_of::<RoutineNodeId>() as u64)
            );
        }
        let distinct = self.rid_global.len() as u64;
        let mut hist = [0u64; 6]; // 1,2,3,4-5,6-10,11+
        for n in self.rid_global.values() {
            let i = match n {
                1 => 0,
                2 => 1,
                3 => 2,
                4..=5 => 3,
                6..=10 => 4,
                _ => 5,
            };
            hist[i] += 1;
        }
        let heap: u64 = self
            .rid_global
            .iter()
            .map(|(r, n)| {
                let h = r.name_lc.len()
                    + r.enclosing_member_lc.as_ref().map_or(0, |s| s.len())
                    + match &r.object.key {
                        ObjKey::Name(n) => n.len(),
                        _ => 0,
                    };
                h as u64 * *n as u64
            })
            .sum();
        println!(
            "  RoutineNodeId: {} copies of {} distinct ({:.2} copies/id); copies histogram 1:{} 2:{} 3:{} 4-5:{} 6-10:{} 11+:{}",
            total,
            distinct,
            total as f64 / distinct.max(1) as f64,
            hist[0],
            hist[1],
            hist[2],
            hist[3],
            hist[4],
            hist[5]
        );
        println!(
            "  RoutineNodeId bytes across copies: inline {:.2} MiB + owned strings {:.2} MiB",
            mibu(total * size_of::<RoutineNodeId>() as u64),
            mibu(heap)
        );
        let mut orows: Vec<_> = self.oid_copies.iter().collect();
        orows.sort_by(|a, b| b.1.cmp(a.1));
        let ot: u64 = orows.iter().map(|x| *x.1).sum();
        println!(
            "  ObjectNodeId: {} copies of {} distinct (incl. the one inside every RoutineNodeId); inline {} B each",
            ot,
            self.oid_global.len(),
            size_of::<ObjectNodeId>()
        );
    }
}

// ───────────────────────── event-edge classes (Q4) ─────────────────────────

fn event_edge_classes(l: &LspSnapshot) -> (u64, u64, u64, u64, Vec<u64>) {
    // (dep->dep only, dep pub with some ws subscriber, ws publisher, total routes to ws) + hashes of dep-only
    let (mut lib, mut mixed, mut ws, mut ws_routes) = (0, 0, 0, 0);
    let mut hashes = Vec::new();
    // S10.4: one edge per publisher, both parts merged (as before the split).
    for e in &l.merged_event_edges() {
        let to_ws = e
            .routine_targets()
            .filter(|id| id.object.app.0 == 0)
            .count() as u64;
        ws_routes += to_ws;
        if e.from.object.app.0 == 0 {
            ws += 1;
        } else if to_ws > 0 {
            mixed += 1;
        } else {
            lib += 1;
            let mut h = DefaultHasher::new();
            e.hash(&mut h);
            hashes.push(h.finish());
        }
    }
    hashes.sort_unstable();
    (lib, mixed, ws, ws_routes, hashes)
}

// ───────────────────────── Q6: alsem analyze's L3 model ─────────────────────────

/// Peak (above the start level) and retained bytes of one call.
fn phase<T>(label: &str, f: impl FnOnce() -> T) -> T {
    settle();
    let (b0, a0) = live();
    reset_peak();
    let t = std::time::Instant::now();
    let v = f();
    let secs = t.elapsed().as_secs_f64();
    let p = PEAK.load(Relaxed);
    let (b1, a1) = live();
    println!(
        "  {label:<52} peak {:>9.1} MiB  live after {:>9.1} MiB ({:>+9} allocs)  {:.1}s",
        mib(p - b0),
        mib(b1 - b0),
        a1 - a0,
        secs
    );
    v
}

/// Per-field drop deltas summed over many values: (bytes, allocs).
#[derive(Default)]
struct Acc(Vec<(&'static str, isize, isize)>);
impl Acc {
    fn drop_as<T>(&mut self, label: &'static str, v: T) {
        let (b0, a0) = live();
        drop(v);
        let (b1, a1) = live();
        match self.0.iter_mut().find(|e| e.0 == label) {
            Some(e) => {
                e.1 += b0 - b1;
                e.2 += a0 - a1;
            }
            None => self.0.push((label, b0 - b1, a0 - a1)),
        }
    }
    fn print(&self, total: isize) {
        let mut rows = self.0.clone();
        rows.sort_by(|a, b| b.1.cmp(&a.1));
        for (l, b, a) in rows {
            println!(
                "    {l:<34} {:>9.2} MiB {:>6.1}% {:>10} allocs",
                mib(b),
                100.0 * b as f64 / total.max(1) as f64,
                a
            );
        }
    }
}

/// #160 §4.1.5: the cross-app analyze path (`build_cross_app_base`, the
/// detector context, `compose_snapshot`) under `ALSEM_RETAINED_CENSUS=1`. The
/// engine prints its bucket lines (stderr); this prints the live heap at every
/// mark, each `census.*` mark taken just before that bucket's walk, so the
/// numbers file can compute `residual = live − Σ buckets` per mark.
fn q6_retained_census(ws: &std::path::Path) {
    use al_sem::engine::gate::run::AnalysisTarget;
    println!(
        "==== Q6 retained census: cross-app analyze on {} ====",
        ws.display()
    );
    MARKS.lock().unwrap().clear();
    let (b0, a0) = live();
    let inputs = al_sem::program::inputs::WorkspaceInputs::read(ws, &Default::default());
    let target = AnalysisTarget::build(&inputs, false, "census-probe").expect("cross-app base");
    on_mark("probe.base_returned");
    if let AnalysisTarget::CrossApp(base) = &target {
        // The default detector set, run by the cross-app runner: builds one
        // detector context (its `census.context` mark fires inside).
        let diags = al_sem::engine::gate::run::compute_analyzer_diagnostics_cross_app(base);
        println!("    analyzer diagnostics: {}", diags.len());
    }
    on_mark("probe.context_dropped");
    let snap = al_sem::engine::l5::snapshot::compose_snapshot(target.resolved());
    on_mark("probe.snapshot_returned");
    println!(
        "    snapshot: {} capability facts, {} typed edges",
        snap.capability_facts.len(),
        snap.typed_edges.len()
    );
    drop(snap);
    drop(target);
    println!(
        "  {:<34} {:>12} {:>12} {:>10}",
        "mark", "live B", "peak B", "allocs"
    );
    for m in MARKS.lock().unwrap().iter() {
        println!(
            "  {:<34} {:>12} {:>12} {:>10}",
            m.name,
            m.live - b0,
            m.peak - b0,
            m.allocs - a0
        );
    }
    MARKS.lock().unwrap().clear();
}

fn q6(ws: PathBuf) {
    use al_sem::program::model::program_calls::assemble_and_resolve_workspace_program;
    use al_sem::program::model::workspace::{Model, ModelEntities, ModelRoutine};
    println!("==== Q6: alsem analyze pipeline on {} ====", ws.display());
    let fresh = phase("fresh_coverage", || {
        let inputs = al_sem::program::inputs::WorkspaceInputs::read(&ws, &Default::default());
        al_sem::program::resolve::full::build_program_with_coverage(&inputs).map(|(_, _, fc)| fc)
    });
    println!("    fresh_coverage ok: {}", fresh.is_ok());
    drop(fresh);
    let id = phase("compute_gate_model_instance_id", || {
        al_sem::engine::gate::model_instance_id::compute_gate_model_instance_id(&ws)
    })
    .expect("model instance id");
    // The production model (engine-switch S6/S9): one program build, the model
    // projected from its parse, the program engine's calls and events attached.
    let resolved = phase("assemble_and_resolve_workspace_program", || {
        assemble_and_resolve_workspace_program(
            &al_sem::program::inputs::WorkspaceInputs::read(&ws, &Default::default()),
            false,
            al_sem::program::model::workspace::AssemblyNeeds::BASE,
        )
    })
    .expect("assemble");
    settle();
    println!(
        "    model: {} objects, {} tables, {} routines; call_sites {}, statement_tree Some {}",
        resolved.workspace.objects.len(),
        resolved.workspace.tables.len(),
        resolved.workspace.routines.len(),
        resolved
            .workspace
            .routines
            .iter()
            .map(|r| r.call_sites.len())
            .sum::<usize>(),
        resolved
            .workspace
            .routines
            .iter()
            .filter(|r| r.statement_tree.is_some())
            .count()
    );
    phase("build_detector_context(ALL) incl. drop", || {
        let ctx = al_sem::engine::l5::detector_context::build_detector_context(
            &resolved,
            al_sem::engine::l5::registry::substrate::ALL,
        );
        let (b, a) = live();
        drop(ctx);
        let (b2, a2) = live();
        println!(
            "    detector context retained (freed on drop): {:.1} MiB in {} allocs",
            mib(b - b2),
            a - a2
        );
    });

    // (a) drop deltas
    settle();
    let (t0, ta0) = live();
    let Model {
        workspace,
        root_classifications,
        primary_app,
        infra_diagnostics,
        workspace_diagnostics,
        dependency_diagnostics,
        calls,
        events,
    } = resolved;
    let ModelEntities {
        objects,
        tables,
        routines,
    } = workspace;
    let mut top = Acc::default();
    let mut rf = Acc::default();
    let nroutines = routines.len();
    let rcap = routines.capacity() * size_of::<ModelRoutine>();
    for r in routines {
        let ModelRoutine {
            id,
            stable_routine_id,
            object_id,
            object_type,
            name,
            kind,
            attributes_parsed,
            app_guid,
            normalized_signature_hash,
            record_variables,
            record_operations,
            field_accesses,
            variables,
            parameters,
            access_modifier,
            return_type,
            call_sites,
            operation_sites,
            statement_tree,
            loops,
            source_anchor,
            identifier_references,
            unreachable_statements,
            var_assignments,
            condition_references,
            enclosing_member,
            originating_object,
            enclosing_member_range,
            entry_temp_guard_receiver,
            ..
        } = r;
        rf.drop_as("id", id);
        rf.drop_as("stable_routine_id", stable_routine_id);
        rf.drop_as("object_id", object_id);
        rf.drop_as("object_type", object_type);
        rf.drop_as("name", name);
        rf.drop_as("kind", kind);
        rf.drop_as("app_guid", app_guid);
        rf.drop_as("normalized_signature_hash", normalized_signature_hash);
        rf.drop_as("source_anchor", source_anchor);
        rf.drop_as("attributes_parsed", attributes_parsed);
        rf.drop_as("parameters", parameters);
        rf.drop_as(
            "access_modifier+return_type",
            (access_modifier, return_type),
        );
        rf.drop_as(
            "enclosing/originating/range/guard",
            (
                enclosing_member,
                originating_object,
                enclosing_member_range,
                entry_temp_guard_receiver,
            ),
        );
        rf.drop_as("variables", variables);
        rf.drop_as("record_variables", record_variables);
        rf.drop_as("record_operations", record_operations);
        rf.drop_as("field_accesses", field_accesses);
        rf.drop_as("call_sites", call_sites);
        rf.drop_as("operation_sites", operation_sites);
        rf.drop_as("statement_tree", statement_tree);
        rf.drop_as("loops", loops);
        rf.drop_as("identifier_references", identifier_references);
        rf.drop_as("unreachable_statements", unreachable_statements);
        rf.drop_as("var_assignments", var_assignments);
        rf.drop_as("condition_references", condition_references);
    }
    // the routines Vec backing itself was freed by the loop's into_iter end
    let (t1, ta1) = live();
    let routines_total = t0 - t1;
    top.drop_as("objects", objects);
    top.drop_as("tables", tables);
    top.drop_as("root_classifications", root_classifications);
    top.drop_as("primary_app", primary_app);
    top.drop_as("infra_diagnostics", infra_diagnostics);
    top.drop_as("workspace_diagnostics", workspace_diagnostics);
    top.drop_as("dependency_diagnostics", dependency_diagnostics);
    top.drop_as("calls (program engine)", calls);
    top.drop_as("events (program engine)", events);
    let (t2, ta2) = live();
    let total = t0 - t2;
    println!(
        "  (a) Model retained: {:.2} MiB in {} allocs; routines {} = {:.2} MiB in {} allocs (Vec<ModelRoutine> backing {:.2} MiB, {} B each)",
        mib(total),
        ta0 - ta2,
        nroutines,
        mib(routines_total),
        ta0 - ta1,
        mib(rcap as isize),
        size_of::<ModelRoutine>()
    );
    println!("  top-level (besides routines):");
    top.print(total);
    println!("  ModelRoutine fields (owned heap freed per field, summed; share of all Model):");
    rf.print(total);
}

/// Process working set, from the OS. CONTEXT ONLY: every byte figure in the
/// report comes from the counting allocator, not from this.
fn rss_context() -> String {
    let pid = std::process::id();
    let cmd = format!(
        "$p=Get-Process -Id {pid}; '{{0:N1}} MiB working set, {{1:N1}} MiB peak working set' -f ($p.WorkingSet64/1MB), ($p.PeakWorkingSet64/1MB)"
    );
    match std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", &cmd])
        .output()
    {
        Ok(o) => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        Err(e) => format!("unavailable ({e})"),
    }
}

/// `--with-updaters`: start each root's updater as `src/server.rs` does (no-op
/// `on_swap`), one at a time, and record the heap delta each one adds once idle.
/// The probe's `ws` (ParsedUnit) is MOVED into the updater, as in the server,
/// so the delta is the updater's own indexes (ResolveIndex, DeclSurface, object
/// map, `Rung1Context`) -- `Rung1Context` is `pub(crate)`, so the split
/// between those three is not reachable from here; only the per-root total is.
fn updaters_mode(
    built: Vec<Root>,
    source: DependencySource,
    cache: &Arc<DepCache>,
    built_live: isize,
) {
    use al_sem::lsp::updater::{ChangeEvent, SharedSnapshot, spawn_updater};
    println!("\n==== --with-updaters: starting one updater per root ====");
    let before_all = live();
    let mut keep = Vec::new();
    let mut tails = Vec::new();
    for (i, r) in built.into_iter().enumerate() {
        let Root {
            path,
            snap,
            ws,
            diags,
        } = r;
        settle();
        let (b0, a0) = live();
        let shared = Arc::new(SharedSnapshot::new(Arc::new(snap)));
        let (tx, rx) = std::sync::mpsc::channel::<ChangeEvent>();
        let handle = spawn_updater(
            Arc::clone(&shared),
            rx,
            path.clone(),
            ws,
            source,
            Arc::clone(cache),
            |_, _| {},
        );
        // Let the updater thread reach its idle `gather_batch` before we settle.
        std::thread::sleep(std::time::Duration::from_millis(600));
        settle();
        let (b1, a1) = live();
        println!(
            "  root {} {}: updater adds {:.2} MiB in {} allocs",
            i + 1,
            path.display(),
            mib(b1 - b0),
            a1 - a0
        );
        keep.push((shared, tx, handle));
        tails.push(diags);
    }
    settle();
    let after_all = live();
    println!(
        "RETAINED, ALL {} UPDATERS IDLE: {:.1} MiB in {} allocs (all-roots-built was {:.1} MiB; updaters add {:.1} MiB in {} allocs)",
        keep.len(),
        mib(after_all.0 - (before_all.0 - built_live)),
        after_all.1,
        mib(built_live),
        mib(after_all.0 - before_all.0),
        after_all.1 - before_all.1
    );
    println!(
        "RSS context (OS, not used for any byte figure): {}",
        rss_context()
    );
    let mut handles = Vec::new();
    for (shared, tx, h) in keep {
        drop(tx);
        handles.push((shared, h));
    }
    for (shared, h) in handles {
        h.join().expect("updater thread");
        drop(shared);
    }
    drop(tails);
    settle();
    println!(
        "after updaters joined and snapshots dropped: live {:.1} MiB",
        mib(live().0)
    );
}

/// `--index-split`: for each root, build the three things the idle updater's
/// `Rung1Context` holds (`ResolveIndex`, workspace object map, `DeclSurface`)
/// from the root's own graph, one at a time, and record the live-heap delta each
/// adds. Then drop the `ResolveIndex` field by field (`census_parts`) and
/// record each drop. Counting-allocator deltas only (requested bytes, hash-map
/// capacity included). The structures are built here by the same calls as
/// `Rung1Context::build`, not taken from a running updater.
fn index_split_mode(built: Vec<Root>) {
    use al_sem::program::resolve::decl_surface::DeclSurface;
    use al_sem::program::resolve::full::app_object_map;
    use al_sem::program::resolve::index::ResolveIndex;
    println!("\n==== --index-split: Rung1Context pieces per root ====");
    let mut sums: Vec<(String, f64, isize)> = Vec::new();
    let mut add = |name: &str, m: f64, a: isize| {
        if let Some(e) = sums.iter_mut().find(|e| e.0 == name) {
            e.1 += m;
            e.2 += a;
        } else {
            sums.push((name.to_string(), m, a));
        }
    };
    let n = built.len();
    for (i, r) in built.iter().enumerate() {
        let graph = &r.snap.graph;
        let primary = graph
            .apps
            .find(&r.snap.snap.workspace_app)
            .expect("workspace app interned");
        println!(
            "\n-- root {} {} : objects {} (own {}), routines {} (own {})",
            i + 1,
            r.path.display(),
            graph.objects.len(),
            graph.objects.own().len(),
            graph.routines.len(),
            graph.routines.own().len()
        );
        settle();
        let (b0, a0) = live();
        let idx = ResolveIndex::build(graph);
        settle();
        let (b1, a1) = live();
        let map = app_object_map(graph, primary);
        settle();
        let (b2, a2) = live();
        let surf = DeclSurface::build(graph, std::slice::from_ref(&r.ws))
            .with_frozen(Arc::clone(&r.snap.dep_meta));
        settle();
        let (b3, a3) = live();
        println!(
            "  ResolveIndex total {:>8.2} MiB {:>8} allocs | object map ({} entries) {:>8.2} MiB {:>6} allocs | DeclSurface (local part) {:>8.2} MiB {:>6} allocs",
            mib(b1 - b0),
            a1 - a0,
            map.len(),
            mib(b2 - b1),
            a2 - a1,
            mib(b3 - b2),
            a3 - a2
        );
        add("ResolveIndex total", mib(b1 - b0), a1 - a0);
        add("object map", mib(b2 - b1), a2 - a1);
        add("DeclSurface local", mib(b3 - b2), a3 - a2);
        println!(
            "  Rung1Context pieces sum {:.2} MiB in {} allocs",
            mib(b3 - b0),
            a3 - a0
        );
        let mut parts_b = 0isize;
        let mut parts_a = 0isize;
        for (name, part) in idx.census_parts() {
            let (pb, pa) = live();
            drop(part);
            let (qb, qa) = live();
            println!(
                "    {name:<26} {:>8.2} MiB {:>8} allocs",
                mib(pb - qb),
                pa - qa
            );
            add(&format!("  {name}"), mib(pb - qb), pa - qa);
            parts_b += pb - qb;
            parts_a += pa - qa;
        }
        println!(
            "    {:<26} {:>8.2} MiB {:>8} allocs  (total minus parts: {:.3} MiB, {} allocs)",
            "sum of parts",
            mib(parts_b),
            parts_a,
            mib((b1 - b0) - parts_b),
            (a1 - a0) - parts_a
        );
        drop(map);
        drop(surf);
    }
    println!("\nSUM over {n} roots (MiB, allocs):");
    for (name, m, a) in &sums {
        println!("  {name:<28} {m:>9.2} MiB {a:>9} allocs");
    }
    println!(
        "RSS context (OS, not used for any byte figure): {}",
        rss_context()
    );
}

/// `routines` dropped field by field, the deltas summed per field.
fn routine_fields(routines: Vec<al_sem::program::model::workspace::ModelRoutine>) {
    use al_sem::program::model::workspace::ModelRoutine;
    let (t0, _) = live();
    let n = routines.len();
    let mut rf = Acc::default();
    for r in routines {
        let ModelRoutine {
            id,
            stable_routine_id,
            object_id,
            object_type,
            name,
            kind,
            attributes_parsed,
            app_guid,
            normalized_signature_hash,
            record_variables,
            record_operations,
            field_accesses,
            variables,
            parameters,
            call_sites,
            operation_sites,
            statement_tree,
            loops,
            site_exprs,
            source_anchor,
            identifier_references,
            unreachable_statements,
            var_assignments,
            condition_references,
            rules,
            rule_drafts,
            card_text,
            ..
        } = r;
        rf.drop_as(
            "ids/names/strings",
            (
                id,
                stable_routine_id,
                object_id,
                object_type,
                name,
                kind,
                app_guid,
                normalized_signature_hash,
                source_anchor,
            ),
        );
        rf.drop_as("attributes_parsed", attributes_parsed);
        rf.drop_as("parameters", parameters);
        rf.drop_as("variables", variables);
        rf.drop_as("record_variables", record_variables);
        rf.drop_as("record_operations", record_operations);
        rf.drop_as("field_accesses", field_accesses);
        rf.drop_as("call_sites", call_sites);
        rf.drop_as("operation_sites", operation_sites);
        rf.drop_as("statement_tree", statement_tree);
        rf.drop_as("loops", loops);
        rf.drop_as("site_exprs", site_exprs);
        rf.drop_as("identifier_references", identifier_references);
        rf.drop_as("unreachable_statements", unreachable_statements);
        rf.drop_as("var_assignments", var_assignments);
        rf.drop_as("condition_references", condition_references);
        rf.drop_as("rules+drafts", (rules, rule_drafts));
        rf.drop_as("card_text", card_text);
        rf.drop_as("rest of the row", ());
    }
    let total = t0 - live().0;
    println!(
        "    {n} routines, {:.1} MiB (size_of ModelRoutine {} B)",
        mib(total),
        size_of::<ModelRoutine>()
    );
    rf.print(total);
}

/// The resident engine's live heap (context-server stage 3, T-mem): a
/// workspace session and one revision built as `alsem engine-host` builds
/// them, then one ask of each served kind (twice), then every retained part
/// dropped in order, each drop delta printed. Live heap only: what the
/// allocator keeps beyond it is the child's RSS minus these numbers.
fn resident_mode(ws: PathBuf, routine: String, low_mem: bool) {
    use al_sem::engine::batch::job::Job;
    use al_sem::engine::resident::revision::Revision;
    use al_sem::engine::resident::session::{REVISION_READ, build_target};
    use al_sem::engine::resident::store::Control;
    use al_sem::program::inputs::WorkspaceInputs;
    use al_sem::program::session::WorkspaceSession;

    let line = |label: &str, b0: isize, t: std::time::Instant| {
        let (b, a) = live();
        println!(
            "  {label:<40} live {:>9.1} MiB  (+{:>8.1})  peak {:>9.1} MiB  allocs {:>10}  {:.1}s",
            mib(b),
            mib(b - b0),
            mib(PEAK.load(Relaxed)),
            a,
            t.elapsed().as_secs_f64()
        );
        reset_peak();
    };
    let base = live().0;
    PEAK.store(base, Relaxed);
    println!(
        "######## resident {} routine {routine}{}",
        ws.display(),
        if low_mem { " --low-mem" } else { "" }
    );
    let t = std::time::Instant::now();
    let inputs = WorkspaceInputs::read(&ws, &REVISION_READ);
    line("read", base, t);
    let mut session = WorkspaceSession::with_mode(if low_mem {
        al_sem::program::session::MemoryMode::LowMem
    } else {
        al_sem::program::session::MemoryMode::Default
    });
    let t = std::time::Instant::now();
    let b0 = live().0;
    let target = build_target(&mut session, &inputs, false)
        .unwrap_or_else(|_| panic!("the model did not build"));
    line("build (session + target)", b0, t);
    let id = al_sem::wire::RevisionId {
        engine_instance_id: "probe".into(),
        generation: 1,
        inputs_stamp: "probe".into(),
    };
    let rev = Revision::new(id, Arc::clone(&inputs), target);
    let ask = |name: &str| -> Box<dyn Job + '_> { resident_job(name, &inputs, &routine) };
    for round in 0..2 {
        for name in ["query", "explain", "prove", "digest"] {
            let t = std::time::Instant::now();
            let b0 = live().0;
            let job = ask(name);
            let out = rev.answer(&*job, &Control::unreserved());
            drop(job);
            let ok = out
                .as_ref()
                .map_or_else(|e| format!("{e:?}"), |o| format!("exit {}", o.exit));
            drop(out);
            line(&format!("ask {name} (round {round}) {ok}"), b0, t);
        }
    }
    println!("  fills: {:?}", rev.fill_counts());
    println!("-- drop deltas (MiB, allocs):");
    {
        use al_sem::engine::resident::revision::{ArtifactKey, ArtifactValue};
        let fold = rev.snapshot().get(&ArtifactKey::Fold).cloned();
        if let Some(fold) = fold {
            rev.evict(&[ArtifactKey::Fold]);
            if let Ok(a) = Arc::try_unwrap(fold)
                && let ArtifactValue::Fold(f) = a.value
            {
                for (name, part) in f.into_census_parts() {
                    if name == "fold.pd" {
                        let pd = part
                            .downcast::<Option<al_sem::engine::l4::pd_solve::ConeArena>>()
                            .expect("the pd part");
                        if let Some(pd) = *pd {
                            println!(
                                "  pd: producers {} states {} (cap {}, {} B) links {} (cap {}, {} B) edges {} by_routine {}",
                                pd.producers.len(),
                                pd.states.len(),
                                pd.states.capacity(),
                                size_of::<al_sem::engine::l4::pd_solve::ConeState>(),
                                pd.links.len(),
                                pd.links.capacity(),
                                size_of::<al_sem::engine::l4::pd_solve::ConeLink>(),
                                pd.edges.len(),
                                pd.by_routine.len()
                            );
                            let al_sem::engine::l4::pd_solve::ConeArena {
                                producers,
                                states,
                                links,
                                edges,
                                by_routine,
                            } = pd;
                            step("fold.pd.by_routine", by_routine);
                            step("fold.pd.links", links);
                            step("fold.pd.states", states);
                            step("fold.pd.edges", edges);
                            step("fold.pd.producers", producers);
                        }
                        continue;
                    }
                    step(name, part);
                }
            }
        }
    }
    for kind in ["run", "det", "raw", "fold", "l4", "rules", "ordering"] {
        let keys = rev.keys_where(|k| k.kind() == kind);
        let (b0, a0) = live();
        rev.evict(&keys);
        let (b1, a1) = live();
        println!(
            "  {:<58} {:>9.2} MiB {:>10} allocs",
            format!("artifact.{kind} ({} keys)", keys.len()),
            mib(b0 - b1),
            a0 - a1
        );
    }
    step("probe's inputs handle", inputs);
    for (name, part) in rev.into_census_parts() {
        if name != "revision.target" {
            step(name, part);
            continue;
        }
        let target = part
            .downcast::<Arc<al_sem::engine::gate::run::AnalysisTarget>>()
            .expect("the target part");
        let Ok(target) = Arc::try_unwrap(*target) else {
            panic!("the target is shared")
        };
        match target {
            al_sem::engine::gate::run::AnalysisTarget::CrossApp(b) => {
                let mut b = *b;
                step("base.leaf_summaries", std::mem::take(&mut b.leaf_summaries));
                step("base.direct_full", std::mem::take(&mut b.direct_full));
                step(
                    "base.direct_coverage",
                    std::mem::take(&mut b.direct_coverage),
                );
                step(
                    "base.dep_routine_ids",
                    std::mem::take(&mut b.dep_routine_ids),
                );
                step("base.fixed_roles", std::mem::take(&mut b.fixed_roles));
                step("base.dependency_calls", b.dependency_calls.take());
                let m = &mut b.resolved;
                step(
                    "model.calls",
                    std::mem::replace(&mut m.calls, Default::default()),
                );
                step(
                    "model.events",
                    std::mem::replace(&mut m.events, Default::default()),
                );
                println!("  model.routines by field:");
                routine_fields(std::mem::take(&mut m.workspace.routines));
                step("model.tables", std::mem::take(&mut m.workspace.tables));
                step("model.objects", std::mem::take(&mut m.workspace.objects));
                step("base rest (rel, field index, coverage, …)", b);
            }
            other => {
                step("target (single-app model)", other);
            }
        }
    }
    for (name, part) in session.take_census_parts() {
        if name == "retained.tier" {
            let tier = part.downcast::<Arc<DepNodes>>().expect("the tier part");
            match Arc::try_unwrap(*tier) {
                Ok(t) => {
                    let DepNodes {
                        objects,
                        routines,
                        dep_meta,
                        bodies,
                        lsp,
                        lsp_events,
                        ..
                    } = t;
                    if let Some(b) = &bodies {
                        let (files, text): (usize, usize) = b
                            .iter()
                            .flat_map(|u| u.files.iter())
                            .fold((0, 0), |(n, t), f| (n + 1, t + f.text.len()));
                        println!(
                            "  tier.bodies: {} units, {files} files, text {:.1} MiB",
                            b.len(),
                            mib(text as isize)
                        );
                    }
                    step("session.tier.bodies", bodies);
                    step("session.tier.routines", routines);
                    step("session.tier.objects", objects);
                    step("session.tier.dep_meta", dep_meta);
                    step("session.tier.lsp", (lsp, lsp_events));
                }
                Err(t) => {
                    step("session.retained.tier (shared)", t);
                }
            };
            continue;
        }
        if name == "rows.dependency.routines" {
            let rows = part
                .downcast::<Vec<Vec<al_sem::program::model::workspace::ModelRoutine>>>()
                .expect("the rows part");
            println!("  session.rows.dependency.routines by field:");
            routine_fields(rows.into_iter().flatten().collect());
            continue;
        }
        if name == "retained.dependency_resolution" {
            use al_sem::program::resolve::full::DependencyBodyResolution;
            let d = part
                .downcast::<Arc<DependencyBodyResolution>>()
                .expect("the dependency part");
            match Arc::try_unwrap(*d) {
                Ok(d) => {
                    let routes: usize = d.edges.iter().map(|e| e.edge.routes.len()).sum();
                    let route_cap: usize = d.edges.iter().map(|e| e.edge.routes.capacity()).sum();
                    println!(
                        "  dep edges: len {} cap {} size_of {}; routes len {routes} cap {route_cap} size_of {}",
                        d.edges.len(),
                        d.edges.capacity(),
                        size_of::<ClassifiedEdge>(),
                        size_of::<Route>()
                    );
                    step("session.dep_resolution.edges", d.edges);
                    step("session.dep_resolution.site_facts", d.site_facts);
                    step("session.dep_resolution.parenless", d.parenless_calls);
                }
                Err(d) => {
                    step("session.dep_resolution (shared)", d);
                }
            }
            continue;
        }
        step(&format!("session.{name}"), part);
    }
    step("session", session);
    println!("  remaining live {:>9.1} MiB", mib(live().0 - base));
}

/// One served ask over `inputs` for `routine`: `query`, `explain`, `prove`,
/// else `digest` (the four kinds `resident` and `refresh-seq` ask).
fn resident_job<'a>(
    name: &str,
    inputs: &'a Arc<al_sem::program::inputs::WorkspaceInputs>,
    routine: &'a str,
) -> Box<dyn al_sem::engine::batch::job::Job + 'a> {
    use al_sem::engine::l5::digest_cli::{DiffRoot, DigestJob, DigestRequest};
    use al_sem::engine::l5::explain::{ExplainJob, ExplainMode, ExplainRequest};
    let ver = "probe";
    match name {
        "query" => Box::new(al_sem::engine::l4::effect_query_cli::QueryJob::effects(
            inputs, routine, ver, true, false,
        )),
        "explain" => Box::new(ExplainJob::new(
            ExplainRequest {
                inputs,
                mode: ExplainMode::Routines(vec![routine.to_string()]),
                single_app: false,
            },
            true,
            None,
        )),
        "prove" => Box::new(al_sem::engine::l5::prove::ProveJob {
            detectors: al_sem::engine::l5::detectors::default_detectors(),
            inputs,
            routine_selector: routine,
            question_text: "may-commit",
            alsem_ver: ver,
            deterministic: true,
            single_app: false,
            closed_world: false,
            human: false,
            out: None,
        }),
        _ => Box::new(DigestJob::new(
            DigestRequest {
                inputs,
                changed_files: None,
                changed_routines: Some(vec![routine.to_string()]),
                diff_text: None,
                alsem_ver: ver,
                deterministic: true,
                single_app: false,
                tests: false,
                include_reads: false,
                diff_root: DiffRoot::Workspace,
            },
            false,
            None,
        )),
    }
}

/// `/proc/self/status` field `key` (e.g. `VmRSS:`), in KiB.
fn proc_status_kib(key: &str) -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with(key))?
                .split_whitespace()
                .nth(1)?
                .parse()
                .ok()
        })
        .unwrap_or(0)
}

/// The resident engine across a sequence of refreshes (the per-refresh
/// memory creep): a `ResidentSession` on a workspace COPY, pinned `fresh`
/// as `alsem engine-host` pins it, then `--b N` body-only edits and `--s N`
/// declaration edits of one codeunit file, each followed by the four asks.
/// Per step: the change class and refresh wall time, the live heap (the
/// counting allocator) after the refresh and after the asks, and the RSS
/// (meaningful with `--features mimalloc`, the child's allocator). The
/// previous revision is released before each refresh, as the child releases
/// it. The workspace must not be inside a git checkout (it is edited).
fn refresh_seq_mode(ws: PathBuf, routine: String, low_mem: bool, nb: usize, ns: usize) {
    use al_sem::engine::resident::session::{Built, Freshness, ResidentSession};
    use al_sem::engine::resident::store::Control;

    LOG_BIG.store(false, Relaxed);
    let ws = ws.canonicalize().expect("the workspace copy");
    let (target, original, steps) = edit_sequence(&ws, nb, ns);
    println!(
        "######## refresh-seq {} routine {routine}{} allocator {} target {}",
        ws.display(),
        if low_mem { " --low-mem" } else { "" },
        if cfg!(feature = "mimalloc") {
            "mimalloc"
        } else {
            "system"
        },
        target.display()
    );
    #[cfg(feature = "mimalloc")]
    // SAFETY: an option store before any concurrent allocation matters; the
    // index is `alsem`'s (`purge_delay`, one below `use_numa_nodes`).
    unsafe {
        libmimalloc_sys::mi_option_set(libmimalloc_sys::mi_option_use_numa_nodes - 1, 0)
    };
    // `PROBE_RELEASE=<bytes>` sets the release threshold, `0` measures plain
    // mimalloc (huge pages left on); by default the child's settings as
    // `alsem engine-host` makes them (`set_resident(RELEASE_MIN, low_mem)`:
    // huge pages off under `--low-mem` only).
    #[cfg(feature = "mimalloc")]
    al_sem::release_alloc::set_resident(
        std::env::var("PROBE_RELEASE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(al_sem::release_alloc::RELEASE_MIN),
        low_mem,
    );
    println!(
        "  release on free: {}, huge pages off: {}",
        al_sem::release_alloc::release_on_free(),
        al_sem::release_alloc::thp_disabled()
    );

    let session =
        Arc::new(ResidentSession::new(ws.clone(), false, "probe".into()).with_low_mem(low_mem));
    let base = live().0;
    println!(
        "  {:<5} {:>5} {:>8} {:>11} {:>11} {:>9} {:>9} {:>9}",
        "step", "class", "refresh", "live-after", "live-asks", "rss", "hwm", "asks"
    );
    let mut first: Option<(isize, u64)> = None;
    for (name, text) in &steps {
        if name != "cold" {
            std::fs::write(&target, text).expect("write the workspace copy");
            session.notify_changed([target.clone()]);
        }
        let t = std::time::Instant::now();
        let pinned = session
            .pin(Freshness::Fresh, &|| None)
            .unwrap_or_else(|e| panic!("{name}: no pin: {e:?}"));
        let refresh_s = t.elapsed().as_secs_f64();
        let Built::Revision(rev) = pinned.built.clone() else {
            panic!("{name}: the build failed")
        };
        drop(pinned);
        settle();
        let live_after = live().0 - base;
        let class = session
            .stats()
            .last_build_class
            .unwrap_or_else(|| "?".into());
        let t = std::time::Instant::now();
        for kind in ["query", "explain", "prove", "digest"] {
            let job = resident_job(kind, &rev.inputs, &routine);
            let out = rev.answer(&*job, &Control::unreserved());
            assert!(out.is_ok(), "{name}: {kind} failed");
        }
        let asks_s = t.elapsed().as_secs_f64();
        drop(rev);
        settle();
        let live_asks = live().0 - base;
        let rss = proc_status_kib("VmRSS:");
        let hwm = proc_status_kib("VmHWM:");
        if first.is_none() {
            first = Some((live_asks, rss));
        }
        let (l0, r0) = first.unwrap();
        println!(
            "  {name:<5} {class:>5} {refresh_s:>7.2}s {:>8.1} MiB {:>8.1} MiB {:>6.2} GB {:>6.2} GB {asks_s:>8.1}s  (live {:+.1} MiB, rss {:+.2} GB vs cold)",
            mib(live_after),
            mib(live_asks),
            rss as f64 / 1e6 * 1.024,
            hwm as f64 / 1e6 * 1.024,
            mib(live_asks - l0),
            (rss as f64 - r0 as f64) / 1e6 * 1.024,
        );
        // Where the RSS beyond the live heap sits: C code (tree-sitter,
        // zstd) allocates through glibc malloc, which the counting
        // allocator never sees.
        let g = glibc_heap();
        let (releases, released) = al_sem::release_alloc::released();
        println!(
            "        glibc held {:.1} MiB in use {:.1} MiB (arena {:.1} mmap {:.1}); released blocks {releases} ({:.1} MiB) so far",
            mibu(g.0),
            mibu(g.1),
            mibu(g.2),
            mibu(g.3),
            mibu(released)
        );
        #[cfg(feature = "mimalloc")]
        if std::env::var_os("PROBE_COLLECT").is_some() {
            // SAFETY: plain mimalloc calls, each on its own thread's heap.
            rayon::broadcast(|_| unsafe { libmimalloc_sys::mi_collect(true) });
            unsafe { libmimalloc_sys::mi_collect(true) };
            println!(
                "        after mi_collect on every global-pool thread and main: rss {:.2} GB",
                proc_status_kib("VmRSS:") as f64 / 1e6 * 1.024
            );
        }
        #[cfg(feature = "mimalloc")]
        if std::env::var_os("PROBE_MI_AREAS").is_some() {
            print!("{}", mi_areas_text());
        }
        #[cfg(feature = "mimalloc")]
        if std::env::var_os("PROBE_MI_STATS").is_some() {
            print!("{}", mi_stats_text());
        }
    }
    std::fs::write(&target, &original).expect("restore the workspace copy");
}

/// Stage 5's measure (context-server design §12.1, "analysis prepared"): a
/// `ResidentSession` on a workspace COPY, pinned `fresh`; per step (`cold`,
/// then a body edit `B`, a declaration edit `S` and a package-identity
/// change `P`, each restored by the next edit): the refresh, then the
/// analysis every heavy request shares filled through `ensure_prepared`
/// (the fold, then the L4 solve, each timed), then each served ask timed.
/// `--verify` compares every ask's bytes and exit with a cold standalone run
/// of the same job over the same read. The previous revision is released
/// before each refresh, as the child releases it. `--steps B,S,P,B` picks
/// the steps after `cold`.
fn incr_mode(ws: PathBuf, routine: String, low_mem: bool, verify: bool, steps: Vec<String>) {
    use al_sem::engine::resident::revision::ArtifactKey;
    use al_sem::engine::resident::session::{Built, Freshness, ResidentSession};
    use al_sem::engine::resident::store::Control;

    LOG_BIG.store(false, Relaxed);
    let ws = ws.canonicalize().expect("the workspace copy");
    let (target, original, _) = edit_sequence(&ws, 0, 0);
    let app_json = ws.join("app.json");
    let app_original = std::fs::read(&app_json).expect("app.json");
    #[cfg(feature = "mimalloc")]
    // SAFETY: as in `refresh_seq_mode`.
    unsafe {
        libmimalloc_sys::mi_option_set(libmimalloc_sys::mi_option_use_numa_nodes - 1, 0)
    };
    #[cfg(feature = "mimalloc")]
    al_sem::release_alloc::set_resident(al_sem::release_alloc::RELEASE_MIN, low_mem);
    println!(
        "######## incr {} routine {routine}{}{} target {}",
        ws.display(),
        if low_mem { " --low-mem" } else { "" },
        if verify { " --verify" } else { "" },
        target.display()
    );
    let session =
        Arc::new(ResidentSession::new(ws.clone(), false, "probe".into()).with_low_mem(low_mem));
    let mut all = vec!["cold".to_string()];
    all.extend(steps);
    let mut failed = false;
    for (i, name) in all.iter().enumerate() {
        // Restore what the previous step changed, then make this step's edit.
        let body = original.replacen(
            "\n    begin\n",
            &format!("\n    begin\n        // incr probe {i}\n"),
            1,
        );
        let close = original.rfind('}').expect("a closing brace");
        let decl = format!(
            "{}\n    procedure IncrProbe{i}()\n    begin\n    end;\n{}",
            &original[..close],
            &original[close..]
        );
        let (text, app): (&str, Vec<u8>) = match name.as_str() {
            "cold" => (&original, app_original.clone()),
            "B" => (&body, app_original.clone()),
            "S" => (&decl, app_original.clone()),
            "P" => {
                let mut a = app_original.clone();
                a.extend(std::iter::repeat_n(b'\n', i));
                (&original, a)
            }
            other => panic!("unknown step {other}"),
        };
        std::fs::write(&target, text).expect("write the workspace copy");
        std::fs::write(&app_json, &app).expect("write app.json");
        session.notify_changed([target.clone(), app_json.clone()]);
        let t = std::time::Instant::now();
        let pinned = session
            .pin(Freshness::Fresh, &|| None)
            .unwrap_or_else(|e| panic!("{name}: no pin: {e:?}"));
        let refresh_ms = t.elapsed().as_millis();
        let Built::Revision(rev) = pinned.built.clone() else {
            panic!("{name}: the build failed")
        };
        drop(pinned);
        let class = session
            .stats()
            .last_build_class
            .unwrap_or_else(|| "?".into());
        let ctl = Control::unreserved();
        let fold_ms = rev
            .prepare_keys(&[ArtifactKey::Fold], &ctl)
            .unwrap_or_else(|e| panic!("{name}: fold: {e:?}"));
        let l4_ms = rev
            .prepare_keys(&[ArtifactKey::L4], &ctl)
            .unwrap_or_else(|e| panic!("{name}: l4: {e:?}"));
        if let Some(p) = rev.pd_report() {
            println!(
                "        pd producers {} reused {} (states {}), delta routines {}",
                p.producers, p.reused, p.reused_states, p.delta_routines
            );
        }
        if let Some((c, (n, with, entries))) = rev.cone_report() {
            println!(
                "        cones: sccs {} refolded {} cut-off {} pd-rows {} (pd delta {}) copied rows {}; store {n} SCCs, {with} cones, {entries} entries",
                c.sccs, c.refolded, c.cut_off, c.pd_rows, c.pd_delta, c.copied_rows
            );
        }
        if let Some((seeded, total, full)) = rev.l4_report() {
            println!(
                "        l4 seeded {seeded} of {total} SCCs{}",
                if full { " (full)" } else { "" }
            );
        }
        let mut asks = Vec::new();
        for kind in ["explain", "prove", "digest", "query"] {
            let job = resident_job(kind, &rev.inputs, &routine);
            let t = std::time::Instant::now();
            let out = rev
                .answer(&*job, &Control::unreserved())
                .unwrap_or_else(|e| panic!("{name}: {kind}: {e:?}"));
            let ms = t.elapsed().as_millis();
            let equal = verify.then(|| {
                let cold = al_sem::engine::batch::standalone_output(&*job);
                cold.exit == out.exit && cold.actions == out.actions
            });
            if equal == Some(false) {
                failed = true;
            }
            asks.push(format!(
                "{kind} {ms} ms{}",
                match equal {
                    Some(true) => " =cold",
                    Some(false) => " DIFFERS",
                    None => "",
                }
            ));
        }
        drop(rev);
        settle();
        println!(
            "  {name:<5} class {class:<4} refresh {refresh_ms:>6} ms  fold {fold_ms:>6} ms  l4 {l4_ms:>5} ms  prepared {:>6} ms  | {} | rss {:.2} GB hwm {:.2} GB",
            fold_ms + l4_ms,
            asks.join(", "),
            proc_status_kib("VmRSS:") as f64 / 1e6 * 1.024,
            proc_status_kib("VmHWM:") as f64 / 1e6 * 1.024,
        );
        // `ALSEM_CONES_CENSUS=1`: the cumulative attribution after each step
        // (a step's own share is the difference from the previous report).
        eprintln!("[cones-census] ---- after step {i} ({name})");
        al_sem::engine::l5::detector_context::report_cones_census();
    }
    std::fs::write(&target, &original).expect("restore the workspace copy");
    std::fs::write(&app_json, &app_original).expect("restore app.json");
    assert!(!failed, "an answer differed from its cold run");
}

/// mimalloc's statistics (`mi_stats_print_out`), as text.
#[cfg(feature = "mimalloc")]
fn mi_stats_text() -> String {
    unsafe extern "C" fn out(msg: *const std::ffi::c_char, arg: *mut std::ffi::c_void) {
        // SAFETY: `arg` is the `String` below; `msg` a C string mimalloc owns.
        let s = unsafe { &mut *(arg as *mut String) };
        s.push_str(&unsafe { std::ffi::CStr::from_ptr(msg) }.to_string_lossy());
    }
    let mut s = String::new();
    // SAFETY: the callback writes only into `s`, alive for the call.
    unsafe {
        libmimalloc_sys::mi_stats_print_out(Some(out), &mut s as *mut String as *mut _);
    }
    s
}

/// mimalloc's pages by block size (`mi_heap_visit_blocks` over the main heap,
/// owned and abandoned pages; run with `MIMALLOC_VISIT_ABANDONED=1`): the
/// committed bytes, the bytes in use, and the waste (committed less in use),
/// the totals and the 12 block sizes that waste the most.
#[cfg(feature = "mimalloc")]
fn mi_areas_text() -> String {
    use libmimalloc_sys::{mi_heap_area_t, mi_heap_t};
    unsafe extern "C" {
        fn mi_heap_main() -> *mut mi_heap_t;
        fn mi_heap_visit_abandoned_blocks(
            heap: *mut mi_heap_t,
            visit_blocks: bool,
            visitor: libmimalloc_sys::mi_block_visit_fun,
            arg: *mut std::ffi::c_void,
        ) -> bool;
    }
    // block size -> (pages, committed, used bytes)
    type Areas = std::collections::BTreeMap<usize, (u64, u64, u64)>;
    unsafe extern "C" fn visit(
        _heap: *const mi_heap_t,
        area: *const mi_heap_area_t,
        _block: *mut std::ffi::c_void,
        _size: usize,
        arg: *mut std::ffi::c_void,
    ) -> bool {
        // SAFETY: `arg` is the map below; `area` is mimalloc's.
        let (m, a) = unsafe { (&mut *(arg as *mut Areas), &*area) };
        let e = m.entry(a.block_size).or_default();
        e.0 += 1;
        e.1 += a.committed as u64;
        e.2 += (a.used * a.block_size) as u64;
        true
    }
    let mut m: Areas = Areas::new();
    // SAFETY: the visitor writes only into `m`, alive for the calls. The
    // visit allocates nothing through the global allocator (the map's
    // inserts do, on this thread, which mimalloc allows during a visit).
    unsafe {
        let h = mi_heap_main();
        libmimalloc_sys::mi_heap_visit_blocks(
            h,
            false,
            Some(visit),
            &mut m as *mut Areas as *mut _,
        );
        mi_heap_visit_abandoned_blocks(h, false, Some(visit), &mut m as *mut Areas as *mut _);
    }
    let (p, c, u) = m
        .values()
        .fold((0, 0, 0), |(p, c, u), e| (p + e.0, c + e.1, u + e.2));
    let mut s = format!(
        "        mi pages {p} committed {:.1} MiB used {:.1} MiB waste {:.1} MiB\n",
        mibu(c),
        mibu(u),
        mibu(c - u)
    );
    let mut by: Vec<_> = m.iter().collect();
    by.sort_by_key(|(_, e)| std::cmp::Reverse(e.1 - e.2));
    for (bs, e) in by.into_iter().take(12) {
        s.push_str(&format!(
            "          block {bs:>8}: pages {:>6} committed {:>8.1} MiB used {:>8.1} MiB waste {:>7.1} MiB\n",
            e.0,
            mibu(e.1),
            mibu(e.2),
            mibu(e.1 - e.2)
        ));
    }
    s
}

/// glibc malloc's own heap (`mallinfo2`, every arena): (held = arena bytes
/// plus mmapped chunks, in use = allocated chunks plus mmapped chunks, arena,
/// mmapped).
fn glibc_heap() -> (u64, u64, u64, u64) {
    #[repr(C)]
    struct Mallinfo2 {
        arena: usize,
        ordblks: usize,
        smblks: usize,
        hblks: usize,
        hblkhd: usize,
        usmblks: usize,
        fsmblks: usize,
        uordblks: usize,
        fordblks: usize,
        keepcost: usize,
    }
    unsafe extern "C" {
        fn mallinfo2() -> Mallinfo2;
    }
    // SAFETY: a plain glibc query.
    let m = unsafe { mallinfo2() };
    (
        (m.arena + m.hblkhd) as u64,
        (m.uordblks + m.hblkhd) as u64,
        m.arena as u64,
        m.hblkhd as u64,
    )
}

/// The edit sequence `refresh-seq` and `front-seq` run on a workspace copy:
/// the target file (the median-sized codeunit file with a procedure body),
/// its original text, and the steps `cold`, `B1..Bnb` (a distinct comment in
/// the first procedure body), `S1..Sns` (a distinct procedure added).
fn edit_sequence(
    ws: &std::path::Path,
    nb: usize,
    ns: usize,
) -> (PathBuf, String, Vec<(String, String)>) {
    fn al_files(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if !p
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with('.'))
                {
                    al_files(&p, out);
                }
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("al")) {
                out.push(p);
            }
        }
    }
    assert!(
        !ws.ancestors().any(|d| d.join(".git").exists()),
        "{} is inside a git checkout: run on a copy",
        ws.display()
    );
    let mut all = Vec::new();
    al_files(ws, &mut all);
    let mut cands: Vec<(u64, PathBuf)> = all
        .into_iter()
        .filter_map(|p| {
            let t = std::fs::read_to_string(&p).ok()?;
            (t.contains("\n    begin\n") && t.trim_start().to_lowercase().starts_with("codeunit"))
                .then_some((t.len() as u64, p))
        })
        .collect();
    cands.sort();
    let target = cands[cands.len() / 2].1.clone();
    let original = std::fs::read_to_string(&target).expect("the target file");
    let mut steps: Vec<(String, String)> = vec![("cold".into(), original.clone())];
    for i in 0..nb {
        steps.push((
            format!("B{}", i + 1),
            original.replacen(
                "\n    begin\n",
                &format!("\n    begin\n        // refresh probe {i}\n"),
                1,
            ),
        ));
    }
    let close = original.rfind('}').expect("a closing brace");
    for j in 0..ns {
        steps.push((
            format!("S{}", j + 1),
            format!(
                "{}\n    procedure RefreshProbe{j}()\n    begin\n    end;\n{}",
                &original[..close],
                &original[close..]
            ),
        ));
    }
    (target, original, steps)
}

/// `refresh-seq`'s sequence against a real `alsem engine-host` child (the
/// resident server as shipped, its own allocator): after each edit a
/// `session/refresh {wait}`, then the four asks over the protocol, then the
/// child's RSS (`status.memory.rssBytes`) and peak (`/proc/<pid>` VmHWM).
fn front_seq_mode(
    alsem: PathBuf,
    ws: PathBuf,
    routine: String,
    low_mem: bool,
    nb: usize,
    ns: usize,
) {
    use al_sem::engine::host::AskParams;
    use al_sem::engine::host::front::{EngineFront, FrontConfig};

    let ws = ws.canonicalize().expect("the workspace copy");
    let (target, original, steps) = edit_sequence(&ws, nb, ns);
    let w = ws.to_str().expect("a UTF-8 path").to_string();
    let params = |argv: &[&str]| -> AskParams {
        let out = std::process::Command::new(&alsem)
            .args(["engine-ask", "--params", "--"])
            .args(argv)
            .output()
            .expect("alsem runs");
        assert!(out.status.success(), "{argv:?}");
        serde_json::from_slice(&out.stdout).expect("the params decode")
    };
    let asks: Vec<(&str, AskParams)> = vec![
        (
            "query",
            params(&[
                "query",
                "effects",
                &w,
                "--routine",
                &routine,
                "--deterministic",
            ]),
        ),
        (
            "explain",
            params(&["explain", &w, "--routine", &routine, "--format", "json"]),
        ),
        (
            "prove",
            params(&["prove", &w, &routine, "may-commit", "--deterministic"]),
        ),
        (
            "digest",
            params(&["digest", &w, "--routine", &routine, "--deterministic"]),
        ),
    ];
    let mut cfg = FrontConfig::new(&alsem, &ws);
    if low_mem {
        cfg.args.push("--low-mem".into());
    }
    let mut front = EngineFront::new(cfg).expect("front end");
    println!(
        "######## front-seq {} {} routine {routine}{} target {}",
        alsem.display(),
        ws.display(),
        if low_mem { " --low-mem" } else { "" },
        target.display()
    );
    println!(
        "  {:<5} {:>5} {:>8} {:>9} {:>9} {:>9} {:>9}",
        "step", "class", "build", "rss", "floor", "hwm", "asks"
    );
    let mut first: Option<u64> = None;
    for (name, text) in &steps {
        if name != "cold" {
            std::fs::write(&target, text).expect("write the workspace copy");
            front
                .notify_changed(vec![target.clone()])
                .expect("notifyChanged");
        }
        front.refresh(true).expect("refresh");
        let st = front.status().expect("status");
        let session = st.session.clone().unwrap_or_default();
        let class = session["lastBuildClass"]
            .as_str()
            .unwrap_or("?")
            .to_string();
        let build_s = session["lastBuildMs"].as_u64().unwrap_or(0) as f64 / 1000.0;
        let t = std::time::Instant::now();
        for (kind, p) in &asks {
            let a = front.ask(p.clone());
            assert!(
                matches!(a.outcome, al_sem::wire::Outcome::Cli { .. }),
                "{name}: {kind}: {:?}",
                a.outcome
            );
        }
        let asks_s = t.elapsed().as_secs_f64();
        std::thread::sleep(std::time::Duration::from_millis(500));
        let st = front.status().expect("status");
        let mem = serde_json::to_value(&st.memory).unwrap_or_default();
        let rss = mem["rssBytes"].as_u64().unwrap_or(0);
        let floor = mem["retainedBytes"].as_u64().unwrap_or(0);
        let hwm = std::fs::read_to_string(format!("/proc/{}/status", st.pid))
            .ok()
            .and_then(|s| {
                s.lines()
                    .find(|l| l.starts_with("VmHWM:"))?
                    .split_whitespace()
                    .nth(1)?
                    .parse::<u64>()
                    .ok()
            })
            .unwrap_or(0)
            * 1024;
        let r0 = *first.get_or_insert(rss);
        println!(
            "  {name:<5} {class:>5} {build_s:>7.2}s {:>6.2} GB {:>6.2} GB {:>6.2} GB {asks_s:>8.1}s  (rss {:+.2} GB vs cold; released {} blocks so far)",
            rss as f64 / 1e9,
            floor as f64 / 1e9,
            hwm as f64 / 1e9,
            (rss as f64 - r0 as f64) / 1e9,
            mem["releasedBlocks"].as_u64().unwrap_or(0)
        );
    }
    std::fs::write(&target, &original).expect("restore the workspace copy");
    front.shutdown();
}

fn main() {
    let mut args = std::env::args().skip(1);
    let source = match args.next().as_deref() {
        Some("resident") => {
            // `resident <ws> [routine] [--low-mem]`
            let rest: Vec<String> = args.collect();
            let low_mem = rest.iter().any(|a| a == "--low-mem");
            let mut plain = rest.into_iter().filter(|a| a != "--low-mem");
            let ws = PathBuf::from(plain.next().expect("workspace"));
            let routine = plain.next().unwrap_or_else(|| "IssueReminder".into());
            resident_mode(ws, routine, low_mem);
            return;
        }
        Some("incr") => {
            // `incr <ws-copy> [routine] [--low-mem] [--verify] [--steps B,S,P]`
            let mut rest = args.peekable();
            let mut plain: Vec<String> = Vec::new();
            let (mut low_mem, mut verify) = (false, false);
            let mut steps: Vec<String> = vec!["B".into(), "S".into(), "P".into()];
            while let Some(a) = rest.next() {
                match a.as_str() {
                    "--low-mem" => low_mem = true,
                    "--verify" => verify = true,
                    "--steps" => {
                        steps = rest
                            .next()
                            .expect("--steps B,S,P")
                            .split(',')
                            .map(str::to_string)
                            .collect()
                    }
                    _ => plain.push(a),
                }
            }
            let mut plain = plain.into_iter();
            let ws = PathBuf::from(plain.next().expect("workspace"));
            let routine = plain.next().unwrap_or_else(|| "IssueReminder".into());
            incr_mode(ws, routine, low_mem, verify, steps);
            return;
        }
        Some(mode @ ("refresh-seq" | "front-seq")) => {
            // `refresh-seq <ws-copy> [routine] [--low-mem] [--b N] [--s N]`
            // `front-seq <alsem> <ws-copy> [routine] [--low-mem] [--b N] [--s N]`
            let front = mode == "front-seq";
            let mut rest = args.peekable();
            let mut plain: Vec<String> = Vec::new();
            let (mut low_mem, mut nb, mut ns) = (false, 20usize, 10usize);
            while let Some(a) = rest.next() {
                match a.as_str() {
                    "--low-mem" => low_mem = true,
                    "--b" => nb = rest.next().expect("--b N").parse().expect("--b N"),
                    "--s" => ns = rest.next().expect("--s N").parse().expect("--s N"),
                    _ => plain.push(a),
                }
            }
            let mut plain = plain.into_iter();
            let alsem = front.then(|| PathBuf::from(plain.next().expect("alsem binary")));
            let ws = PathBuf::from(plain.next().expect("workspace"));
            let routine = plain.next().unwrap_or_else(|| "IssueReminder".into());
            match alsem {
                Some(alsem) => front_seq_mode(alsem, ws, routine, low_mem, nb, ns),
                None => refresh_seq_mode(ws, routine, low_mem, nb, ns),
            }
            return;
        }
        Some("embedded") => DependencySource::Embedded,
        Some("symbols") => DependencySource::Symbols,
        // Q6, the analyze model's cost (was `l3` before engine-switch S9).
        Some("model") => {
            q6(PathBuf::from(args.next().expect("workspace")));
            return;
        }
        // #160 §4.1.5, its own mode: the census flag and the mark hook are
        // process `OnceLock`s, and `on_mark` resets PEAK, so either would skew
        // `model`'s phase peaks and seconds.
        Some("retained") => {
            // Before the engine's first `census::enabled()` read (a OnceLock).
            // SAFETY: still single-threaded; no other thread reads the environment.
            unsafe { std::env::set_var("ALSEM_RETAINED_CENSUS", "1") };
            al_sem::census_hook::HOOK.set(on_mark).unwrap();
            q6_retained_census(&PathBuf::from(args.next().expect("workspace")));
            return;
        }
        _ => panic!("usage: census-probe <embedded|symbols|model|retained> <root>..."),
    };
    let rest: Vec<String> = args.collect();
    let with_updaters = rest.iter().any(|a| a == "--with-updaters");
    let roots: Vec<PathBuf> = rest
        .iter()
        .filter(|a| *a != "--with-updaters" && *a != "--index-split")
        .map(PathBuf::from)
        .collect();
    al_sem::census_hook::HOOK.set(on_mark).unwrap();

    println!(
        "size_of: RoutineNodeId={} ObjectNodeId={} RoutineNode={} ObjectNode={} RoutineMeta={} ClassifiedEdge={} Route={} LspEdge={} LspTarget={} DeclEntry={} EdgeRef={} ParsedFileEntry={}",
        size_of::<RoutineNodeId>(),
        size_of::<ObjectNodeId>(),
        size_of::<RoutineNode>(),
        size_of::<ObjectNode>(),
        size_of::<RoutineMeta>(),
        size_of::<ClassifiedEdge>(),
        size_of::<Route>(),
        size_of::<LspEdge>(),
        size_of::<LspTarget>(),
        size_of::<DeclEntry>(),
        size_of::<EdgeRef>(),
        size_of::<ParsedFileEntry>()
    );

    let cache = Arc::new(DepCache::default());
    let start = live();
    let mut built: Vec<Root> = Vec::new();
    for (i, root) in roots.iter().enumerate() {
        settle();
        MARKS.lock().unwrap().clear();
        PHASE.store(0, Relaxed);
        ROOT.store(i + 1, Relaxed);
        let (b0, a0) = live();
        reset_peak();
        let t = std::time::Instant::now();
        let (snap, ws) = LspSnapshot::build_full_with_parsed_with_cache(root, source, &cache)
            .unwrap_or_else(|_| panic!("build failed: {}", root.display()));
        on_mark("10.returned");
        let secs = t.elapsed().as_secs_f64();
        println!(
            "\n######## root {} = {} ({:?}) built in {:.1}s",
            i + 1,
            root.display(),
            source,
            secs
        );
        println!(
            "Q3 phases (MiB relative to live heap before this root's build; peak = max live inside the phase):"
        );
        ROOT.store(0, Relaxed);
        let mut overall = 0;
        let mut peak_phase = "";
        let mut names: Vec<&'static str> = Vec::new();
        for m in MARKS.lock().unwrap().iter() {
            names.push(m.name);
            if m.peak > overall {
                overall = m.peak;
                peak_phase = m.name;
            }
            println!(
                "  {:<40} end-live {:>9.1}  in-phase peak {:>9.1}  live allocs {:>+10}",
                m.name,
                mib(m.live - b0),
                mib(m.peak - b0),
                m.allocs - a0
            );
        }
        settle();
        let (b1, a1) = live();
        println!(
            "  BUILD PEAK {:.1} MiB; retained after settle {:.1} MiB in {} allocs",
            mib(overall - b0),
            mib(b1 - b0),
            a1 - a0
        );
        println!("  build peak is in phase {peak_phase}");
        let symbol_only = snap.snap.apps.iter().filter(|u| u.source.is_none());
        println!(
            "  SymbolOnly units ingested by step 2b (source None): {} ({} with an app_path); abi_ingest_errors {}",
            symbol_only.clone().count(),
            symbol_only.filter(|u| u.app_path.is_some()).count(),
            snap.dep_layer.abi_ingest_errors.len()
        );
        report_bigs(i + 1, &names, b0);
        let cfg = al_sem::config::DiagnosticConfig::load(root);
        let d = al_sem::lsp::diagnostics::compute_all(&snap, PositionEncoding::Utf16, &cfg);
        let diags: Box<dyn std::any::Any> = Box::new(d);
        let (b2, a2) = live();
        println!(
            "  + diagnostics map {:.2} MiB in {} allocs",
            mib(b2 - b1),
            a2 - a1
        );
        println!(
            "  shape: objects {} (shared {} own {}), routines {} (shared {} own {}), dep_meta {} (orphans {}), dep_lines {}, ws files {}, edges_by_file edges {}, event_edges {}, incoming keys {}, decl_by_id {}",
            snap.graph.objects.len(),
            snap.graph.objects.shared().len(),
            snap.graph.objects.own().len(),
            snap.graph.routines.len(),
            snap.graph.routines.shared().len(),
            snap.graph.routines.own().len(),
            snap.dep_meta.len(),
            snap.dep_meta.orphan_count(),
            snap.dep_lines.len(),
            snap.parsed.len(),
            snap.edges_by_file.values().map(|v| v.len()).sum::<usize>(),
            snap.event_edges().count(),
            snap.all_incoming().len(),
            snap.decl_by_id.len()
        );
        let (lib, mixed, wsn, wsr, h) = event_edge_classes(&snap);
        let same_as_first = built.first().map(|r0| event_edge_classes(&r0.snap).4 == h);
        println!(
            "  event_edges classes: dep->dep only {} | dep publisher w/ ws subscriber {} | ws publisher {} | routes into ws {} | dep->dep set identical to root 1: {:?}",
            lib, mixed, wsn, wsr, same_as_first
        );
        if let Some(r0) = built.first() {
            println!(
                "  sharing vs root 1: dep_nodes ptr_eq {} | dep_meta ptr_eq {} | dep_lines ptr_eq {} | objects.shared ptr_eq {}",
                Arc::ptr_eq(&r0.snap.dep_layer.dep_nodes, &snap.dep_layer.dep_nodes),
                Arc::ptr_eq(&r0.snap.dep_meta, &snap.dep_meta),
                Arc::ptr_eq(&r0.snap.dep_lines, &snap.dep_lines),
                Arc::ptr_eq(snap.graph.objects.shared(), r0.snap.graph.objects.shared()),
            );
        }
        built.push(Root {
            path: root.clone(),
            snap,
            ws,
            diags,
        });
    }
    settle();
    let (bt, at) = live();
    println!(
        "\nALL {} ROOTS LIVE: {:.1} MiB in {} allocs",
        built.len(),
        mib(bt - start.0),
        at - start.1
    );

    if rest.iter().any(|a| a == "--index-split") {
        index_split_mode(built);
        return;
    }
    if with_updaters {
        updaters_mode(built, source, &cache, bt - start.0);
        return;
    }

    // Q2: walk root 1 alone, then all roots (Arc-deduped). Walk memory is freed before the drop steps.
    {
        let mut w = W::default();
        w.snapshot(&built[0].snap, &built[0].ws);
        w.report(&format!("root 1 only ({})", built[0].path.display()));
    }
    if built.len() > 1 {
        let mut w = W::default();
        for r in &built {
            w.snapshot(&r.snap, &r.ws);
        }
        w.report(&format!(
            "all {} roots (shared Arcs counted once)",
            built.len()
        ));
    }
    settle();

    // Q1/Q4/Q5: drop roots in REVERSE order -> roots N..2 show their UNIQUE cost,
    // root 1 (dropped last) shows the shared tier too.
    println!("\n==== drop deltas (reverse build order) ====");
    while let Some(r) = built.pop() {
        drop_root_by_field(r);
        settle();
    }
    drop(cache);
    let end = live();
    println!(
        "residual after all roots + DepCache dropped: {:.2} MiB in {} allocs",
        mib(end.0 - start.0),
        end.1 - start.1
    );
}
