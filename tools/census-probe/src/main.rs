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

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::mem::size_of;
use std::path::PathBuf;
use std::sync::atomic::{AtomicIsize, Ordering::Relaxed};
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

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let n = BYTES.fetch_add(l.size() as isize, Relaxed) + l.size() as isize;
        PEAK.fetch_max(n, Relaxed);
        ALLOCS.fetch_add(1, Relaxed);
        unsafe { System.alloc(l) }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        let n = BYTES.fetch_add(l.size() as isize, Relaxed) + l.size() as isize;
        PEAK.fetch_max(n, Relaxed);
        ALLOCS.fetch_add(1, Relaxed);
        unsafe { System.alloc_zeroed(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        BYTES.fetch_sub(l.size() as isize, Relaxed);
        ALLOCS.fetch_sub(1, Relaxed);
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        let n = BYTES.fetch_add(new as isize - l.size() as isize, Relaxed) + new as isize
            - l.size() as isize;
        PEAK.fetch_max(n, Relaxed);
        unsafe { System.realloc(p, l, new) }
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

fn q6(ws: PathBuf) {
    use al_sem::program::model::program_calls::assemble_and_resolve_workspace_program;
    use al_sem::program::model::workspace::{Model, ModelEntities, ModelRoutine};
    println!("==== Q6: alsem analyze pipeline on {} ====", ws.display());
    let fresh = phase("fresh_coverage", || {
        al_sem::program::resolve::full::build_program_with_coverage(&ws).map(|(_, _, fc)| fc)
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
        assemble_and_resolve_workspace_program(&ws, &id, false)
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

fn main() {
    let mut args = std::env::args().skip(1);
    let source = match args.next().as_deref() {
        Some("embedded") => DependencySource::Embedded,
        Some("symbols") => DependencySource::Symbols,
        // Q6, the analyze model's cost (was `l3` before engine-switch S9).
        Some("model") => {
            q6(PathBuf::from(args.next().expect("workspace")));
            return;
        }
        _ => panic!("usage: census-probe <embedded|symbols> <root>..."),
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
        let (b0, a0) = live();
        reset_peak();
        let t = std::time::Instant::now();
        let (snap, ws) = LspSnapshot::build_full_with_parsed_with_cache(root, source, &cache)
            .unwrap_or_else(|| panic!("build failed: {}", root.display()));
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
        let mut overall = 0;
        for m in MARKS.lock().unwrap().iter() {
            overall = overall.max(m.peak);
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
